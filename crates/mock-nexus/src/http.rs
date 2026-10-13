//! The request pipeline: read one request, apply the scenario gates, answer, close.

use std::io::{self, BufReader, Read};
use std::net::TcpStream;
use std::time::Duration;

use super::service_api::{
    assets_page, audit_litter_page, base_of, seed_repositories_doc, EULA_DISCLAIMER,
};
use super::storage::{drift_or_store, range_span, transform_put};
use super::support::{
    answer_bad_request, bodyless_for_head, bump, forbidden, log_request, normalize_path, plain,
    query_param, unauthorized,
};
use crate::group;
use crate::scenario::Scenario;
use crate::server::{self, BodyError, BodyPlan, Request, Resp};
use crate::store::{lock, Outcome, Shared};

/// How long the freeze-upload handler holds a PUT connection before dropping it.
/// A bounded hold keeps a long-lived hosted instance from leaking threads and sockets.
const FREEZE_HORIZON_SECS: u64 = 900;

/// How far the cut-body `Content-Length` lies high under `fake_length`.
const FAKE_LENGTH_LIE: usize = 1024;

/// How far the oversized-head drain may read before giving the peer no further courtesy.
const DRAIN_CAP: usize = 256 * 1024;

/// A single read or write may stall at most this long before we drop the peer.
pub(crate) const IO_TIMEOUT: Duration = Duration::from_secs(30);

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
    if shared.spec.freeze_upload && req.method == "PUT" {
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
        if shared.spec.drop_connection && bump(&mut seq, &path) == 0 {
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
        // auth-401 challenges with 401, auth-403 refuses with 403.
        // Neither body rides a HEAD.
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
    // The served names are the ones the seeded service document lists.
    // A search without a repository parameter is left alone.
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

    // service-overview scenarios: the server endpoints answer before the storage store is consulted.
    // Each arm writes its response and returns. Storage scenarios fall through to the match below.
    if matches!(req.method.as_str(), "GET" | "HEAD") {
        let service = match path {
            "service/rest/v1/status" => match shared.scenario {
                Scenario::ServiceStatusEmpty => Some(plain(200, b"", None)),
                Scenario::ServiceStatusVersion => {
                    Some(plain(200, br#"{"version": "3.79.1-04"}"#, None))
                }
                Scenario::ServiceStatusDown => Some(plain(503, b"down\n", None)),
                _ => None,
            },
            "service/rest/v1/status/writable" => match shared.scenario {
                Scenario::ServiceStatusEmpty | Scenario::ServiceStatusVersion => {
                    Some(plain(200, b"", None))
                }
                _ => None,
            },
            "service/rest/v1/system/eula" => match shared.scenario {
                Scenario::EulaAbsent => Some(plain(404, b"not found\n", None)),
                Scenario::EulaGate => {
                    let accepted = lock(&shared.eula_accepted);
                    Some(plain(
                        200,
                        format!(
                            "{{\"accepted\": {}, \"disclaimer\": \"{}\"}}",
                            *accepted, EULA_DISCLAIMER
                        )
                        .as_bytes(),
                        None,
                    ))
                }
                _ => None,
            },
            p if p.starts_with("service/rest/v1/repositories/") => match &shared.scenario {
                Scenario::RepoDetailAdmin { user, pass } => {
                    let expected = format!(
                        "Basic {}",
                        crate::base64::encode(format!("{user}:{pass}").as_bytes())
                    );
                    let ok = req.header("Authorization") == Some(expected.as_str());
                    let name = p.rsplit('/').next().unwrap_or_default();
                    if ok && name == "raw-main" {
                        Some(plain(
                            200,
                            br#"{"name":"raw-main","format":"raw","type":"hosted","url":"http://127.0.0.1/repository/raw-main","online":true,"storage":{"blobStoreName":"default","strictContentTypeValidation":false,"writePolicy":"ALLOW_ONCE"}}"#,
                            None,
                        ))
                    } else {
                        Some(plain(403, b"nx-admin required\n", None))
                    }
                }
                _ => None,
            },
            "service/rest/v1/repositories" => match &shared.scenario {
                Scenario::RepoCollectionScoped { user, pass } => {
                    let expected = format!(
                        "Basic {}",
                        crate::base64::encode(format!("{user}:{pass}").as_bytes())
                    );
                    let ok = req.header("Authorization") == Some(expected.as_str());
                    let body = if ok {
                        let doc = seed_repositories_doc(shared);
                        match doc.rfind(']') {
                            Some(idx) => format!(
                                "{},{},{}]",
                                &doc[..idx],
                                serde_json::json!({
                                    "name": "raw-secret",
                                    "format": "raw",
                                    "type": "hosted",
                                    "url": format!("{}/repository/raw-secret/", base_of(shared)),
                                    "size": 0,
                                    "attributes": {}
                                }),
                                serde_json::json!({
                                    "name": "raw-main",
                                    "format": "raw",
                                    "type": "hosted",
                                    "url": format!("{}/repository/raw-main/", base_of(shared)),
                                    "size": 0,
                                    "attributes": {}
                                })
                            ),
                            None => doc,
                        }
                    } else {
                        seed_repositories_doc(shared)
                    };
                    Some(plain(200, body.as_bytes(), None))
                }
                _ => None,
            },
            "service/rest/v1/search/assets" => match shared.scenario {
                Scenario::AssetsPagination | Scenario::AssetsPrefixQ => {
                    Some(assets_page(shared, req))
                }
                Scenario::AuditLitter => Some(audit_litter_page(shared)),
                Scenario::AssetsAbsent => Some(plain(404, b"not found\n", None)),
                _ => None,
            },
            _ => None,
        };
        if let Some(resp) = service {
            log_request(shared, &req.method, path, Outcome::Status(resp.status));
            let _ = server::write_response(stream, &bodyless_for_head(&req.method, resp));
            return;
        }
    }

    // eula-gate: the acceptance POST echoes the disclaimer back and opens the gate (204).
    // Idempotent: a second POST still answers 204.
    if matches!(shared.scenario, Scenario::EulaGate)
        && req.method == "POST"
        && path == "service/rest/v1/system/eula"
    {
        let raw = String::from_utf8_lossy(&req.body).into_owned();
        let opened =
            raw.contains(EULA_DISCLAIMER) && raw.contains("\"accepted\"") && raw.contains("true");
        log_request(
            shared,
            &req.method,
            path,
            Outcome::Status(if opened { 204 } else { 500 }),
        );
        if opened {
            *lock(&shared.eula_accepted) = true;
            let resp = plain(204, b"", None);
            let _ = server::write_response(stream, &resp);
        } else {
            let resp = plain(500, b"Invalid EULA disclaimer\n", None);
            let _ = server::write_response(stream, &resp);
        }
        return;
    }

    // eula-gate: writes are refused with the EULA body until the gate is opened by echoing the disclaimer.
    if matches!(shared.scenario, Scenario::EulaGate)
        && matches!(req.method.as_str(), "PUT" | "POST" | "DELETE")
        && !path.starts_with("service/rest/v1/system/eula")
    {
        let opened = *lock(&shared.eula_accepted);
        if !opened {
            log_request(shared, &req.method, path, Outcome::Status(403));
            let body = b"You must accept the End User License Agreement (EULA) through the onboarding wizard or REST API before proceeding. See https://links.sonatype.com/products/nxrm3/docs/ce-onboarding for details.";
            let resp = plain(403, body, None);
            let _ = server::write_response(stream, &bodyless_for_head(&req.method, resp));
            return;
        }
    }

    // detail-in-hint: writes answer 403 with a short server sentence, so the client hint carries it.
    if matches!(shared.scenario, Scenario::DetailInHint)
        && matches!(req.method.as_str(), "PUT" | "POST" | "DELETE")
    {
        log_request(shared, &req.method, path, Outcome::Status(403));
        let resp = plain(403, b"please ask the administrator\n", None);
        let _ = server::write_response(stream, &bodyless_for_head(&req.method, resp));
        return;
    }

    match req.method.as_str() {
        "GET" => {
            if let Some(payload) = drift_or_store(shared, path) {
                let total = payload.len();
                let range = if shared.spec.ignore_range {
                    None
                } else {
                    range_span(req.header("Range"))
                };
                match range {
                    // Start at or past the end: nothing to serve from.
                    Some((start, _)) if start >= total => {
                        log_request(shared, &req.method, path, Outcome::Status(416));
                        let mut resp = plain(416, b"range not satisfiable\n", shared.drip);
                        resp.extra_headers
                            .push(("Content-Range", format!("bytes */{total}")));
                        let _ = server::write_response(stream, &resp);
                    }
                    // A recognized range (open or closed): serve the span as Partial Content.
                    Some((start, end)) => {
                        let stop = end.map_or(total - 1, |e| e.min(total - 1));
                        let body = payload[start..=stop].to_vec();
                        log_request(shared, &req.method, path, Outcome::Status(206));
                        let resp = Resp {
                            status: 206,
                            extra_headers: vec![(
                                "Content-Range",
                                format!("bytes {start}-{stop}/{total}"),
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
            if shared.spec.read_only {
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

/// Decide Basic auth: the header must be `Basic <token>` with the expected token.
fn auth_decision(header: Option<&str>, expected_b64: &str) -> bool {
    let Some(value) = header else { return false };
    let Some((scheme, token)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("basic") && token.trim() == expected_b64
}
