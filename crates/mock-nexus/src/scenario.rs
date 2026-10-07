//! Per-scenario behavior: the scenario table, connection handling, per-path state machines and the pure transforms (marker zeroing, document drift, auth decision).

use std::collections::HashMap;
use std::io::{self, BufReader, Read};
use std::net::TcpStream;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::PoisonError;
use std::time::Duration;

use crate::group;
use crate::server::{self, BodyError, BodyPlan, Drip, Request, Resp};
use crate::store::{lock, Outcome, ReqLog, Shared};

/// Scenario names understood by the `mock-nexus` binary and by test harnesses.
pub const SCENARIOS: &[&str] = &[
    "atomic",
    "partial-put",
    "cut-body",
    "drop-connection",
    "freeze-upload",
    "sizeless",
    "slow",
    "foreign-marker",
    "markerless",
    "auth-401",
    "auth-403",
    "doc-drift",
    "flaky",
    "rate-limit",
    "redirect",
    "readonly",
    "no-service",
    "search-400",
];

/// Failure scenario a [`MockNexus`](crate::MockNexus) server simulates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scenario {
    /// Straight-through storage: every fully-read request is served normally.
    Atomic,
    /// The first PUT per path is cut off after `first_attempt_bytes` body bytes: the connection closes with no response and nothing is stored.
    /// Later PUT attempts on that path are read fully and stored.
    PartialPut { first_attempt_bytes: usize },
    /// The first success (200) GET per path writes honest status and headers, then only `after_bytes` body bytes and closes the connection.
    /// With `fake_length` the `Content-Length` header lies [`FAKE_LENGTH_LIE`] bytes high, so the honest-looking answer still breaks mid-body.
    /// Later GETs serve the object whole, so a retry resuming from the part file completes the download.
    CutBody {
        after_bytes: usize,
        fake_length: bool,
    },
    /// The first request per path (any method) is reset right after its head (request line and headers) is read: the body is never read, nothing is answered and nothing is stored.
    /// Later requests are served normally.
    DropConnection,
    /// PUT connections are held right after the head: the body is never read and no response is ever written, so the write side must detect the stall.
    /// The hold is bounded (about 15 minutes), then the connection is dropped.
    /// GET/HEAD behave like [`Scenario::Atomic`].
    FreezeUpload,
    /// Success (200) GET and HEAD answers for present objects carry no `Content-Length`: the proxy case.
    /// A client must refuse to treat such objects as absent (the digest comparison would be skipped).
    /// Every other answer (206, 416, 404) keeps `Content-Length`.
    /// PUTs behave like [`Scenario::Atomic`].
    Sizeless,
    /// GET/HEAD bodies are written in `chunk_size` pieces, sleeping `chunk_delay_ms` between pieces.
    /// PUT bodies are read normally.
    Slow {
        chunk_delay_ms: u64,
        chunk_size: usize,
    },
    /// Stored `.sha256` markers get their 64-char digest replaced by 64 zeros (digest of a foreign object).
    /// Everything else is stored verbatim.
    ForeignMarker,
    /// `.sha256` markers are acknowledged (201 Created) but never stored.
    /// Non-marker bytes are stored normally.
    Markerless,
    /// Every request requires `Authorization: Basic base64(user:pass)`.
    /// Otherwise 401 with `WWW-Authenticate: Basic realm="nexus"`.
    /// Valid credentials behave like [`Scenario::Atomic`].
    Auth401 { user: String, pass: String },
    /// Every request requires `Authorization: Basic base64(user:pass)`.
    /// Anything else answers `403 Forbidden` with a `forbidden` body, on any method (unlike [`Scenario::Auth401`], which answers `401`).
    /// Valid credentials behave like [`Scenario::Atomic`].
    Auth403 { user: String, pass: String },
    /// Behaves like [`Scenario::Atomic`] until [`MockNexus::enable_drift`](crate::MockNexus::enable_drift).
    /// Afterwards every GET of a `*/version.json` path serves a synthesized version document with a ghost artifact.
    /// PUTs keep storing verbatim, and [`MockNexus::disable_drift`](crate::MockNexus::disable_drift) restores store-backed responses.
    DocDrift,
    /// The first `first_failures` requests per path (any method) get 503 Service Unavailable.
    /// Later requests are served normally.
    Flaky { first_failures: u32 },
    /// The first `first_429s` requests per path (any method) answer `429 Too Many Requests` with a `Retry-After: <retry_after_secs>` header and an empty body.
    /// Later requests are served like [`Scenario::Atomic`].
    RateLimit {
        first_429s: usize,
        retry_after_secs: u64,
    },
    /// GET and HEAD requests answer `301 Moved Permanently` with `Location: <location_path>` (same host, the path from the repository root) and an empty body.
    /// Every other method behaves like [`Scenario::Atomic`].
    Redirect { location_path: String },
    /// Every DELETE is refused with `403 Forbidden`: the read-only repository.
    /// Nothing is ever removed from the store.
    /// GET/HEAD/PUT behave like [`Scenario::Atomic`].
    ReadOnly,
    /// The service REST endpoint (`/service/rest/v1/repositories`) answers `404` like a store miss:
    /// an installation without the management API (an old Nexus, or a non-Sonatype server).
    /// Storage behavior is [`Scenario::Atomic`]; only `service repos` notices.
    NoService,
    /// The search API (`/service/rest/v1/search/assets`) answers `400 Bad Request` when the `repository` query parameter names a repository the instance does not serve:
    /// a real Nexus refuses a repository-scoped search for an unknown repository with 400, before any storage is touched.
    /// Searches for the served repositories (`raw-main`, `raw-all`, the names of the seeded service document) and every other request behave like [`Scenario::Atomic`].
    Search400,
}

/// A single read or write may stall at most this long before we drop the peer.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the freeze-upload handler holds a PUT connection before dropping it.
/// A bounded hold keeps a long-lived hosted instance from leaking threads and sockets.
const FREEZE_HORIZON_SECS: u64 = 900;

/// How far the cut-body `Content-Length` lies high under `fake_length`.
const FAKE_LENGTH_LIE: usize = 1024;

/// How far the oversized-head drain may read before giving the peer no further courtesy.
const DRAIN_CAP: usize = 256 * 1024;

/// Serve exactly one request on `stream`, then close the connection.
pub(crate) fn serve(shared: &Shared, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut stream = stream;

    let head = match server::read_head_raw(&mut stream) {
        Ok(Some(head)) => head,
        // An oversized head (past the bound in `read_head_raw`) is a recognizable refusal: answer 400 instead of hanging up silently.
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            answer_bad_request(shared, &mut stream, "", "");
            // The head's tail still sits unread in the receive queue: closing now would reset the connection and destroy the queued 400, so drain first.
            drain(&mut stream);
            return;
        }
        // Clean EOF, timeout or reset mid-head: nothing sensible to answer.
        _ => return,
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

    // freeze-upload: a PUT connection is held without reading the body or answering, so the write side must detect the stall.
    // The handler parks in one-second steps for a bounded window, then drops the socket: a long-lived hosted instance must not leak threads and file descriptors forever.
    // Parked handlers die with the test process long before the horizon.
    if matches!(shared.scenario, Scenario::FreezeUpload) && req.method == "PUT" {
        log_request(shared, &req.method, &path, Outcome::Stalled);
        for _ in 0..FREEZE_HORIZON_SECS {
            std::thread::sleep(Duration::from_secs(1));
        }
        return;
    }

    // drop-connection: the first request per path is reset.
    // The head reader left the terminator's final byte unread on the socket, so dropping the stream now makes the kernel send RST instead of FIN.
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
    handle(shared, &mut stream, &req, &path);
}

/// Read the leftover request bytes up to [`DRAIN_CAP`] so the close is a clean FIN and the peer can read the answer.
/// The read timeout drops to half a second: once the queue is empty the peer sends nothing more, and waiting out the full [`IO_TIMEOUT`] would stall the test for nothing.
fn drain(stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut sink = [0u8; 8192];
    let mut drained = 0usize;
    while drained < DRAIN_CAP {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

/// Apply scenario gates and serve the request.
fn handle(shared: &Shared, stream: &mut TcpStream, req: &Request, path: &str) {
    // A group instance forwards instead of serving its own store: reads walk the members, writes refuse.
    if !shared.members.is_empty() {
        if matches!(req.method.as_str(), "GET" | "HEAD") {
            group::get_first(shared, stream, req, path);
        } else {
            group::refuse_writes(shared, stream, req, path);
        }
        return;
    }

    // auth-401/auth-403: unauthenticated callers are rejected before anything else.
    let unauthenticated = shared
        .auth_b64
        .as_ref()
        .is_some_and(|expected| !auth_decision(req.header("Authorization"), expected));
    if unauthenticated {
        // auth-401 challenges with 401, auth-403 refuses with 403; neither body rides a HEAD.
        let forbidden403 = matches!(shared.scenario, Scenario::Auth403 { .. });
        let resp = if forbidden403 {
            forbidden()
        } else {
            unauthorized()
        };
        log_request(shared, &req.method, path, Outcome::Status(resp.status));
        let _ = server::write_response(stream, &bodyless_for_head(&req.method, resp));
        return;
    }

    // flaky: the first K requests per path get 503.
    if shared.flaky_first.is_some_and(|k| {
        let mut seq = lock(&shared.first_request);
        bump(&mut seq, path) < k
    }) {
        log_request(shared, &req.method, path, Outcome::Status(503));
        let _ = server::write_response(
            stream,
            &bodyless_for_head(&req.method, plain(503, b"flaky\n", None)),
        );
        return;
    }

    // rate-limit: the first N requests per path get 429 with a Retry-After hint and an empty body.
    if let Some((first_429s, retry_after_secs)) = shared.rate_first {
        let seen = {
            let mut seq = lock(&shared.first_request);
            bump(&mut seq, path)
        };
        if seen < first_429s as u32 {
            log_request(shared, &req.method, path, Outcome::Status(429));
            let mut resp = plain(429, b"", None);
            resp.extra_headers
                .push(("Retry-After", retry_after_secs.to_string()));
            let _ = server::write_response(stream, &resp);
            return;
        }
    }

    // redirect: reads point elsewhere with an empty 301, writes stay atomic.
    if let Scenario::Redirect { location_path } = &shared.scenario {
        if matches!(req.method.as_str(), "GET" | "HEAD") {
            log_request(shared, &req.method, path, Outcome::Status(301));
            let mut resp = plain(301, b"", None);
            resp.extra_headers.push(("Location", location_path.clone()));
            let _ = server::write_response(stream, &resp);
            return;
        }
    }

    // search-400: a repository-scoped search for an unknown repository is refused with 400, like a real Nexus.
    // The served names are the ones the seeded service document lists; a search without a repository parameter is left alone.
    if let Scenario::Search400 = shared.scenario {
        if matches!(req.method.as_str(), "GET" | "HEAD") && path == "service/rest/v1/search/assets"
        {
            if let Some(repo) = query_param(&req.target, "repository") {
                if !matches!(repo, "raw-main" | "raw-all") {
                    log_request(shared, &req.method, path, Outcome::Status(400));
                    let body = format!("unknown repository: {repo}\n");
                    let resp = plain(400, body.as_bytes(), None);
                    let _ = server::write_response(stream, &bodyless_for_head(&req.method, resp));
                    return;
                }
            }
        }
    }

    match req.method.as_str() {
        "GET" => {
            if let Some(payload) = drift_or_store(shared, path) {
                let total = payload.len();
                match range_start(req.header("Range")) {
                    // Start at or past the end: nothing to resume from.
                    Some(start) if start >= total => {
                        log_request(shared, &req.method, path, Outcome::Status(416));
                        let mut resp = plain(416, b"range not satisfiable\n", shared.drip);
                        resp.extra_headers
                            .push(("Content-Range", format!("bytes */{total}")));
                        let _ = server::write_response(stream, &resp);
                    }
                    // Single open range: serve the suffix as Partial Content.
                    Some(start) => {
                        let body = payload[start..].to_vec();
                        log_request(shared, &req.method, path, Outcome::Status(206));
                        let resp = Resp {
                            status: 206,
                            extra_headers: vec![(
                                "Content-Range",
                                format!("bytes {start}-{}/{total}", total - 1),
                            )],
                            content_length: body.len(),
                            body,
                            drip: shared.drip,
                            hide_length: false,
                        };
                        let _ = server::write_response(stream, &resp);
                    }
                    // No (recognized) Range: the whole object.
                    None => {
                        log_request(shared, &req.method, path, Outcome::Status(200));
                        // cut-body: the first success GET per path ships honest headers, a truncated body, then closes.
                        if let Some((after_bytes, fake_length)) = cut_plan(shared, path) {
                            let taken = after_bytes.min(total);
                            let mut resp = plain(200, &payload[..taken], shared.drip);
                            resp.content_length = if fake_length {
                                total + FAKE_LENGTH_LIE
                            } else {
                                total
                            };
                            let _ = server::write_response(stream, &resp);
                            return;
                        }
                        let resp = Resp {
                            status: 200,
                            extra_headers: Vec::new(),
                            content_length: total,
                            body: payload,
                            drip: shared.drip,
                            hide_length: shared.sizeless,
                        };
                        let _ = server::write_response(stream, &resp);
                    }
                }
            } else {
                log_request(shared, &req.method, path, Outcome::Status(404));
                let _ = server::write_response(stream, &plain(404, b"not found\n", shared.drip));
            }
        }
        "HEAD" => {
            let found = lock(&shared.store).get(path).map(Vec::len);
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
                hide_length: shared.sizeless && status == 200,
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
        "DELETE" => {
            // readonly: the repository refuses every deletion before the store is touched.
            if matches!(shared.scenario, Scenario::ReadOnly) {
                log_request(shared, &req.method, path, Outcome::Status(403));
                let _ = server::write_response(stream, &plain(403, b"read-only\n", None));
                return;
            }
            // Straight-through storage: an existing object is removed with 204,
            // an absent one answers 404, which keeps deletion idempotent.
            let existed = lock(&shared.store).remove(path).is_some();
            let (status, body): (u16, &[u8]) = if existed {
                (204, b"")
            } else {
                (404, b"not found\n")
            };
            log_request(shared, &req.method, path, Outcome::Status(status));
            let _ = server::write_response(stream, &plain(status, body, None));
        }
        _ => {
            log_request(shared, &req.method, path, Outcome::Status(405));
            let _ = server::write_response(
                stream,
                &bodyless_for_head(&req.method, plain(405, b"method not allowed\n", None)),
            );
        }
    }
}

/// Decide how much of the body to read before the request is processed.
///
/// Under `partial-put` the first PUT per path is cut short.
/// Consuming that "first attempt" happens here, before any bytes of the body are read.
fn body_plan(shared: &Shared, method: &str, path: &str) -> BodyPlan {
    let ("PUT", Some(cut)) = (method, shared.partial_first_put) else {
        return BodyPlan::Full;
    };
    let mut seq = lock(&shared.first_put);
    if bump(&mut seq, path) == 0 {
        BodyPlan::Partial(cut)
    } else {
        BodyPlan::Full
    }
}

/// cut-body gate: `Some((after_bytes, fake_length))` for the first success GET per path, `None` afterwards.
/// Like the partial-put gate, the attempt is consumed when it fires, before the response is written.
fn cut_plan(shared: &Shared, path: &str) -> Option<(usize, bool)> {
    let (cut, fake) = shared.cut_first_get?;
    let mut seq = lock(&shared.first_get);
    (bump(&mut seq, path) == 0).then_some((cut, fake))
}

/// Strip the body bytes off a response aimed at a HEAD request (RFC 9110 §9.3.2): the wire keeps `Content-Length` at the body length but sends nothing after the head.
fn bodyless_for_head(method: &str, mut resp: Resp) -> Resp {
    if method == "HEAD" {
        resp.body = Vec::new();
    }
    resp
}

/// GET payload: the synthesized drifting version document once drift is enabled, otherwise whatever is in the store.
fn drift_or_store(shared: &Shared, path: &str) -> Option<Vec<u8>> {
    if shared.drift.load(Relaxed) && is_version_path(path) {
        return Some(drift_doc(first_segment(path)));
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

pub(crate) fn plain(status: u16, body: &[u8], drip: Option<Drip>) -> Resp {
    Resp {
        status,
        extra_headers: Vec::new(),
        content_length: body.len(),
        body: body.to_vec(),
        drip,
        hide_length: false,
    }
}

fn unauthorized() -> Resp {
    Resp {
        status: 401,
        extra_headers: vec![("WWW-Authenticate", "Basic realm=\"nexus\"".to_owned())],
        content_length: b"auth required\n".len(),
        body: b"auth required\n".to_vec(),
        drip: None,
        hide_length: false,
    }
}

/// The auth-403 refusal: a plain verdict with no challenge.
fn forbidden() -> Resp {
    plain(403, b"forbidden\n", None)
}

fn answer_bad_request(shared: &Shared, stream: &mut TcpStream, method: &str, path: &str) {
    log_request(shared, method, path, Outcome::Status(400));
    let _ = server::write_response(stream, &plain(400, b"bad request\n", None));
}

pub(crate) fn log_request(shared: &Shared, method: &str, path: &str, outcome: Outcome) {
    shared
        .log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
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

/// First `name` value of the request target's query string, raw (percent-encoding preserved).
/// Absent parameter or bare key yields `None`.
fn query_param<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Parse a `Range` header for resumable GETs.
/// Only the single open form `bytes=N-` (from byte `N` to the end) is recognized.
/// Anything else, whether another unit, multiple ranges, the closed `N-M` or suffix `-N` forms, or malformed values, yields `None` and the response is served in full.
fn range_start(header: Option<&str>) -> Option<usize> {
    let value = header?;
    let (unit, spec) = value.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return None;
    }
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if !end.trim().is_empty() {
        return None;
    }
    start.trim().parse().ok()
}

/// True when the path designates a version document (`<version>/version.json`).
fn is_version_path(path: &str) -> bool {
    path.ends_with("/version.json")
}

/// First `/`-separated segment of a store path (the version directory).
fn first_segment(path: &str) -> &str {
    path.split('/').next().unwrap_or_default()
}

/// Synthesized drifting version document, same shape as protocol §4.1 but listing a ghost artifact instead of the real ones.
fn drift_doc(version: &str) -> Vec<u8> {
    format!("{{\"schema_version\":1,\"version\":\"{version}\",\"artifacts\":[\"ghost.zip\"]}}")
        .into_bytes()
}

/// Replace the 64-char digest of the marker's first line with 64 zeros (simulating a sibling written for a foreign object).
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
        auth_decision, drift_doc, first_segment, is_version_path, normalize_path, range_start,
        zero_digest,
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
    fn drift_doc_has_foreign_artifact_and_path_version() {
        assert_eq!(
            drift_doc("1.14.0"),
            br#"{"schema_version":1,"version":"1.14.0","artifacts":["ghost.zip"]}"#.to_vec()
        );
    }

    #[test]
    fn version_paths_and_segments() {
        assert!(is_version_path("1.14.0/version.json"));
        assert!(!is_version_path("1.14.0/version.json.sha256"));
        assert!(!is_version_path("version.json"));
        assert_eq!(first_segment("1.14.0/version.json"), "1.14.0");
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

    #[test]
    fn range_start_accepts_only_single_open_bytes_range() {
        assert_eq!(range_start(Some("bytes=5-")), Some(5));
        assert_eq!(range_start(Some("bytes=0-")), Some(0));
        // Unit comparison is case-insensitive.
        assert_eq!(range_start(Some("Bytes=12-")), Some(12));
        // Closed, suffix and multi-range forms are not supported.
        assert_eq!(range_start(Some("bytes=0-4")), None);
        assert_eq!(range_start(Some("bytes=-5")), None);
        assert_eq!(range_start(Some("bytes=0-4,10-")), None);
        assert_eq!(range_start(Some("items=5-")), None);
        assert_eq!(range_start(Some("bytes=")), None);
        assert_eq!(range_start(Some("bytes=q-")), None);
        assert_eq!(range_start(None), None);
    }
}
