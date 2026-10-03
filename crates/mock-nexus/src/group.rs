//! Group repositories: read-only aggregations that forward to member instances.
//!
//! The dispatch mirrors the real Nexus `GroupHandler` view handler:
//! only `GET`/`HEAD` reach the members, every other method is refused with `405` and `Allow: GET,HEAD`.
//! The member walk relays the first `2xx` and never surfaces a member's own failure to the client.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

use crate::scenario;
use crate::server::{self, Request};
use crate::store::{Outcome, Shared};

/// Forward the request along `shared.members` in order and relay the first `2xx` answer.
/// A member's `404`, `5xx` or broken connection is skipped without a retry and without reaching the client.
/// When no member answers `2xx`, the standard single-instance `404` goes out.
pub(crate) fn get_first(shared: &Shared, stream: &mut TcpStream, req: &Request, path: &str) {
    let request = forward_bytes(req);
    for addr in &shared.members {
        if let Some((status, response)) = relay_member(*addr, &request) {
            scenario::log_request(shared, &req.method, path, Outcome::Status(status));
            // The member bytes already carry status line, headers, `Content-Length` and `Connection: close`.
            let _ = stream.write_all(&response);
            return;
        }
    }
    scenario::log_request(shared, &req.method, path, Outcome::Status(404));
    let _ = server::write_response(stream, &scenario::plain(404, b"not found\n", None));
}

/// Refuse a write through the group before any member is contacted.
/// The wire shape follows real Nexus: `405`, `Allow: GET,HEAD` (joined without a space) and no body.
pub(crate) fn refuse_writes(shared: &Shared, stream: &mut TcpStream, req: &Request, path: &str) {
    scenario::log_request(shared, &req.method, path, Outcome::Status(405));
    let mut resp = scenario::plain(405, b"", None);
    resp.extra_headers.push(("Allow", "GET,HEAD".to_owned()));
    let _ = server::write_response(stream, &resp);
}

/// Relay `request` to one member and return its status and bytes when the status is `2xx`.
/// A transport failure or any non-2xx answer yields `None`: the member is skipped.
fn relay_member(addr: SocketAddr, request: &[u8]) -> Option<(u16, Vec<u8>)> {
    let mut member = TcpStream::connect(addr).ok()?;
    let _ = member.set_read_timeout(Some(scenario::IO_TIMEOUT));
    let _ = member.set_write_timeout(Some(scenario::IO_TIMEOUT));
    member.write_all(request).ok()?;
    // Members answer one request per connection and close, so EOF ends the answer.
    let mut response = Vec::new();
    member.read_to_end(&mut response).ok()?;
    let status = status_of(&response)?;
    (200..300).contains(&status).then_some((status, response))
}

/// The request bytes a member sees: the client's request line and headers as received, with the body re-framed by `Content-Length`.
/// The body was fully read (and de-chunked) before dispatch, so the framing headers are replaced rather than forwarded.
fn forward_bytes(req: &Request) -> Vec<u8> {
    let mut out = format!("{} {} HTTP/1.1\r\n", req.method, req.target).into_bytes();
    for (name, value) in &req.headers {
        let framing = name.eq_ignore_ascii_case("content-length")
            || name.eq_ignore_ascii_case("transfer-encoding");
        if !framing {
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
    }
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", req.body.len()).as_bytes());
    out.extend_from_slice(&req.body);
    out
}

/// The status code of a raw response: the middle token of the status line.
fn status_of(response: &[u8]) -> Option<u16> {
    let line = String::from_utf8_lossy(response.split(|b| *b == b'\n').next()?);
    line.split_whitespace().nth(1)?.parse().ok()
}
