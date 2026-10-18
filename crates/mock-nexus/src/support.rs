//! Small shared helpers: canned responses, request logging, counters, path and header parsing.
//! The request pipeline in `http` and the REST pages in `service_api` read them.

use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::PoisonError;

use crate::server::{self, Drip, Resp};
pub(super) use crate::store::lock;
use crate::store::{Outcome, ReqLog, Shared};

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

pub(super) fn unauthorized() -> Resp {
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
pub(super) fn forbidden() -> Resp {
    plain(403, b"forbidden\n", None)
}

pub(super) fn answer_bad_request(
    shared: &Shared,
    stream: &mut TcpStream,
    method: &str,
    path: &str,
) {
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
pub(super) fn bump(counters: &mut HashMap<String, u32>, path: &str) -> u32 {
    let seen = counters.entry(path.to_owned()).or_insert(0);
    let prev = *seen;
    *seen += 1;
    prev
}

/// Store key for a raw request target: query string stripped, no leading '/'.
/// Percent-encoding is preserved (never decoded).
pub(super) fn normalize_path(target: &str) -> String {
    let no_query = target.split('?').next().unwrap_or(target);
    no_query.strip_prefix('/').unwrap_or(no_query).to_owned()
}

/// First `name` value of the request target's query string, raw (percent-encoding preserved).
/// Absent parameter or bare key yields `None`.
pub(super) fn query_param<'a>(target: &'a str, name: &str) -> Option<&'a str> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// Decode the `%XX` escapes of a query value, the way the server hands the parameter to search.
/// A malformed escape stays verbatim: a broken client gets an empty answer, never a panic.
pub(super) fn percent_decode(value: &str) -> String {
    let raw = value.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hex(raw[i + 1]), hex(raw[i + 2])) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Strip the body bytes off a response aimed at a HEAD request (RFC 9110 §9.3.2): the wire keeps `Content-Length` at the body length but sends nothing after the head.
pub(super) fn bodyless_for_head(method: &str, mut resp: Resp) -> Resp {
    if method == "HEAD" {
        resp.body = Vec::new();
    }
    resp
}
