//! Per-scenario behavior: connection handling, per-path state machines and
//! the pure transforms (marker zeroing, claim drift, auth decision).

use std::collections::HashMap;
use std::io::{self, BufReader, Read};
use std::net::TcpStream;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;

use crate::server::{self, BodyError, BodyPlan, Drip, Request, Resp};
use crate::{lock, Outcome, ReqLog, Scenario, Shared};

/// A single read or write may stall at most this long before we drop the peer.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Serve exactly one request on `stream`, then close the connection.
pub(crate) fn serve(shared: &Shared, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut stream = stream;

    let head = match server::read_head_raw(&mut stream) {
        Ok(Some(head)) => head,
        Ok(None) => return,
        Err(_) => return, // timeout or reset mid-head: nothing sensible to answer
    };
    let mut req = match server::parse_head(&head) {
        Ok(req) => req,
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            answer_bad_request(shared, &mut stream, "", "");
            return;
        }
        Err(_) => return,
    };
    let path = normalize_path(&req.target);

    // drop-connection: the first request per path is reset. The head reader
    // left the terminator's final byte unread on the socket, so dropping the
    // stream now makes the kernel send RST instead of FIN.
    {
        let mut seq = lock(&shared.first_request);
        if matches!(shared.scenario, Scenario::DropConnection) && bump(&mut seq, &path) == 0 {
            log_request(shared, &req.method, &path, Outcome::Reset);
            return;
        }
    }

    // Consume the reserved terminator byte so body reads resume at the body.
    let mut reserved = [0u8; 1];
    if stream.read_exact(&mut reserved).is_err() {
        return;
    }
    let Ok(sock) = stream.try_clone() else { return };
    let mut reader = BufReader::new(sock);

    let plan = body_plan(shared, &req.method, &path);
    if let Err(err) = server::read_body_into(&mut reader, &mut req, plan) {
        match err {
            BodyError::Malformed => answer_bad_request(shared, &mut stream, &req.method, &path),
            BodyError::Aborted(received) => {
                // Client aborted the body (or we cut it): no response, nothing stored.
                let outcome = if received == 0 {
                    Outcome::Reset
                } else {
                    Outcome::PartialRead { bytes: received }
                };
                log_request(shared, &req.method, &path, outcome);
            }
        }
        return;
    }
    handle(shared, &mut stream, req, &path);
}

/// Apply scenario gates and serve the request.
fn handle(shared: &Shared, stream: &mut TcpStream, req: Request, path: &str) {
    // auth-401: unauthenticated callers are rejected before anything else.
    let unauthenticated = shared
        .auth_b64
        .as_ref()
        .is_some_and(|expected| !auth_decision(req.header("Authorization"), expected));
    if unauthenticated {
        log_request(shared, &req.method, path, Outcome::Status(401));
        let _ = server::write_response(stream, &unauthorized());
        return;
    }

    // flaky: the first K requests per path get 503.
    if shared.flaky_first.is_some_and(|k| {
        let mut seq = lock(&shared.first_request);
        bump(&mut seq, path) < k
    }) {
        log_request(shared, &req.method, path, Outcome::Status(503));
        let _ = server::write_response(stream, &plain(503, b"flaky\n", None));
        return;
    }

    match req.method.as_str() {
        "GET" => match drift_or_store(shared, path) {
            Some(body) => {
                log_request(shared, &req.method, path, Outcome::Status(200));
                let resp = Resp {
                    status: 200,
                    extra_headers: Vec::new(),
                    content_length: body.len(),
                    body,
                    drip: shared.drip,
                };
                let _ = server::write_response(stream, &resp);
            }
            None => {
                log_request(shared, &req.method, path, Outcome::Status(404));
                let _ = server::write_response(stream, &plain(404, b"not found\n", shared.drip));
            }
        },
        "HEAD" => {
            let found = lock(&shared.store).get(path).map(|b| b.len());
            let (status, content_length) = match found {
                Some(len) => (200, len),
                None => (404, b"not found\n".len()),
            };
            log_request(shared, &req.method, path, Outcome::Status(status));
            let resp = Resp {
                status,
                extra_headers: Vec::new(),
                content_length,
                body: Vec::new(),
                drip: shared.drip,
            };
            let _ = server::write_response(stream, &resp);
        }
        "PUT" => {
            if let Some(bytes) = transform_put(shared, path, &req.body) {
                lock(&shared.store).insert(path.to_owned(), bytes);
            }
            log_request(shared, &req.method, path, Outcome::Status(201));
            let _ = server::write_response(stream, &plain(201, b"", None));
        }
        _ => {
            log_request(shared, &req.method, path, Outcome::Status(405));
            let _ = server::write_response(stream, &plain(405, b"method not allowed\n", None));
        }
    }
}

/// Decide how much of the body to read before the request is processed.
///
/// Under `partial-put` the first PUT per path is cut short.
/// Consuming that "first attempt" happens here, before any bytes of the body are read.
fn body_plan(shared: &Shared, method: &str, path: &str) -> BodyPlan {
    let cut = match (method, shared.partial_first_put) {
        ("PUT", Some(cut)) => cut,
        _ => return BodyPlan::Full,
    };
    let mut seq = lock(&shared.first_put);
    if bump(&mut seq, path) == 0 {
        BodyPlan::Partial(cut)
    } else {
        BodyPlan::Full
    }
}

/// GET payload: the synthesized drifting claim once drift is enabled,
/// otherwise whatever is in the store.
fn drift_or_store(shared: &Shared, path: &str) -> Option<Vec<u8>> {
    if shared.drift.load(Relaxed) && is_claim_path(path) {
        return Some(drift_claim(first_segment(path)));
    }
    lock(&shared.store).get(path).cloned()
}

/// Apply marker mutations for `.sha256` PUTs.
/// `None` means "acknowledge with 201 but store nothing" (markerless).
fn transform_put(shared: &Shared, path: &str, body: &[u8]) -> Option<Vec<u8>> {
    if !path.ends_with(".sha256") {
        return Some(body.to_vec());
    }
    match shared.scenario {
        Scenario::Markerless => None,
        Scenario::ForeignMarker => Some(zero_digest(body)),
        _ => Some(body.to_vec()),
    }
}

fn plain(status: u16, body: &[u8], drip: Option<Drip>) -> Resp {
    Resp {
        status,
        extra_headers: Vec::new(),
        content_length: body.len(),
        body: body.to_vec(),
        drip,
    }
}

fn unauthorized() -> Resp {
    Resp {
        status: 401,
        extra_headers: vec![("WWW-Authenticate", "Basic realm=\"nexus\"".to_owned())],
        content_length: b"auth required\n".len(),
        body: b"auth required\n".to_vec(),
        drip: None,
    }
}

fn answer_bad_request(shared: &Shared, stream: &mut TcpStream, method: &str, path: &str) {
    log_request(shared, method, path, Outcome::Status(400));
    let _ = server::write_response(stream, &plain(400, b"bad request\n", None));
}

fn log_request(shared: &Shared, method: &str, path: &str, outcome: Outcome) {
    shared
        .log
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(ReqLog {
            method: method.to_owned(),
            path: path.to_owned(),
            outcome,
        });
}

/// Return this path's previous counter value and increment it.
fn bump(counters: &mut HashMap<String, u32>, path: &str) -> u32 {
    let seen = counters.entry(path.to_owned()).or_insert(0);
    let prev = *seen;
    *seen += 1;
    prev
}

/// Store key for a raw request target: query string stripped, no leading '/'.
/// Percent-encoding is preserved (never decoded).
fn normalize_path(target: &str) -> String {
    let no_query = target.split('?').next().unwrap_or(target);
    no_query.strip_prefix('/').unwrap_or(no_query).to_owned()
}

/// True when the path designates a claim document (`<version>/claim.json`).
fn is_claim_path(path: &str) -> bool {
    path.ends_with("/claim.json")
}

/// First `/`-separated segment of a store path (the version directory).
fn first_segment(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

/// Synthesized drifting claim, same shape as protocol §4.1 but listing a
/// ghost artifact instead of the real ones.
fn drift_claim(version: &str) -> Vec<u8> {
    format!("{{\"claim_version\":1,\"version\":\"{version}\",\"artifacts\":[\"ghost.zip\"]}}")
        .into_bytes()
}

/// Replace the 64-char digest of the marker's first line with 64 zeros
/// (simulating a sibling written for a foreign object).
/// Everything else is stored verbatim.
/// Shorter payloads pass through unchanged.
fn zero_digest(marker: &[u8]) -> Vec<u8> {
    let line_end = marker
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(marker.len());
    let mut out = marker.to_vec();
    if line_end >= 64 {
        out[..64].fill(b'0');
    }
    out
}

/// Decide Basic auth: the header must be `Basic <token>` with the expected token.
fn auth_decision(header: Option<&str>, expected_b64: &str) -> bool {
    let Some(value) = header else { return false };
    let Some((scheme, token)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("basic") && token.trim() == expected_b64
}

#[cfg(test)]
mod tests {
    use super::{
        auth_decision, drift_claim, first_segment, is_claim_path, normalize_path, zero_digest,
    };

    #[test]
    fn zeroes_64_char_digest_of_marker() {
        let marker = format!("{}  sample.zip\n", "a".repeat(64));
        let out = zero_digest(marker.as_bytes());
        assert_eq!(
            out,
            format!("{}  sample.zip\n", "0".repeat(64)).into_bytes()
        );
    }

    #[test]
    fn short_marker_is_stored_verbatim() {
        assert_eq!(zero_digest(b"tooshort\n"), b"tooshort\n");
    }

    #[test]
    fn drift_claim_has_foreign_artifact_and_path_version() {
        assert_eq!(
            drift_claim("1.14.0"),
            br#"{"claim_version":1,"version":"1.14.0","artifacts":["ghost.zip"]}"#.to_vec()
        );
    }

    #[test]
    fn claim_paths_and_segments() {
        assert!(is_claim_path("1.14.0/claim.json"));
        assert!(!is_claim_path("1.14.0/claim.json.sha256"));
        assert!(!is_claim_path("claim.json"));
        assert_eq!(first_segment("1.14.0/claim.json"), "1.14.0");
    }

    #[test]
    fn normalize_strips_query_and_leading_slash() {
        assert_eq!(normalize_path("/v/a%20b?x=1"), "v/a%20b");
        assert_eq!(normalize_path("plain"), "plain");
    }

    #[test]
    fn auth_decision_table() {
        assert!(auth_decision(Some("Basic Y2k6c2VjcmV0"), "Y2k6c2VjcmV0"));
        // Scheme comparison is case-insensitive.
        assert!(auth_decision(Some("basic Y2k6c2VjcmV0"), "Y2k6c2VjcmV0"));
        assert!(!auth_decision(None, "Y2k6c2VjcmV0"));
        assert!(!auth_decision(Some("Bearer Y2k6c2VjcmV0"), "Y2k6c2VjcmV0"));
        assert!(!auth_decision(Some("Basic d3Jvbmc="), "Y2k6c2VjcmV0"));
        assert!(!auth_decision(Some("Basic"), "Y2k6c2VjcmV0"));
    }
}
