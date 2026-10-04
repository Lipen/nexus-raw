//! Mock Nexus raw-storage server with a failure-scenario table.
//!
//! Implements just enough HTTP/1.1 (std only) to exercise the nexus-raw transport contract.
//! `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, `DELETE` with 204/404.
//! Percent-encoded paths stored verbatim.
//! One request per connection, `Connection: close` on every response.
//! GET honors resumable downloads: a single open `Range: bytes=N-` is answered with `206` and `Content-Range`, out-of-range starts get `416`.
//! Any other `Range` form is ignored.
//! A [`Scenario`] selects a failure mode: truncated PUT and GET bodies, connection resets, held uploads, missing `Content-Length`, slow links, drifted documents, flaky 503s, rate-limit 429s with `Retry-After`, redirects, and Basic-auth gating.
//!
//! Rust conformance tests use the library API directly.
//! The `mock-nexus` binary exposes the same scenarios to shell- and Python-driven tests:
//!
//! ```text
//! let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic)?;
//! let url = format!("{}1.14.0/version.json", server.base_url());
//! ```
//!
//! A group repository is a separate deployment kind, not a scenario:
//! [`MockNexus::start_group`] aggregates two or more running members, forwarding reads in member order and refusing writes.
//! On a group handle the store mutators ([`MockNexus::insert`], [`MockNexus::enable_drift`]) are no-ops: a group stores nothing.

mod base64;
mod group;
mod scenario;
mod server;
mod store;
#[cfg(test)]
mod tests;

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use scenario::{Scenario, SCENARIOS};
pub use store::{Outcome, ReqLog};

use store::{lock, Shared};

/// Handle to a running mock server.
/// Dropping it stops the accept loop and releases the port.
/// In-flight connection threads finish on their own.
#[derive(Debug)]
pub struct MockNexus {
    shared: Arc<Shared>,
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl MockNexus {
    /// Bind a server on a random free port on 127.0.0.1.
    pub fn start(scenario: Scenario) -> std::io::Result<Self> {
        Self::start_on(scenario, SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    /// Like [`MockNexus::start`] but bound to `addr` (the binary's `--port`).
    pub fn start_on(scenario: Scenario, addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        let bound = listener.local_addr()?;
        let shared = Arc::new(Shared::hosted(scenario));
        let stop = Arc::new(AtomicBool::new(false));
        spawn_acceptor(listener, Arc::clone(&shared), Arc::clone(&stop));
        Ok(Self {
            shared,
            addr: bound,
            stop,
        })
    }

    /// Start a group repository over `members` (two or more running instances), listening on its own port on 127.0.0.1.
    ///
    /// Reads (`GET`/`HEAD`) are forwarded to the members' real HTTP endpoints in member order, and the first member answering `2xx` is relayed to the client.
    /// Any other member answer (a `404`, a `5xx`, a broken connection) is skipped: the walk never retries a member and never surfaces a member's failure to the client.
    /// When no member answers `2xx`, the group answers `404` with the same `not found` body as a single-instance miss (real Nexus's `notFound()` sends no body, the mock stays consistent with its own single-repo shape).
    /// Every other method is refused with `405`, `Allow: GET,HEAD` and an empty body: a group is a read-only aggregation, and publication targets hosted members.
    /// Member scenario gates apply to forwarded requests, because the members serve them for real: a flaky member's `503` costs the group nothing once a later member holds the object.
    ///
    /// Limitations, chosen to keep the mock std-only and faithful to the real `GroupHandler` dispatch: members are flat (no nested groups), every member is assumed online, the format-specific escape hatches are not modeled, and the search API is not dispatched (a separate subsystem in real Nexus, so a group URL answers `404` for it).
    ///
    /// # Errors
    ///
    /// Returns an [`io::Error`] of kind [`io::ErrorKind::InvalidInput`] for fewer than two members, and the bind error when the listener cannot open.
    pub fn start_group(members: &[&MockNexus]) -> std::io::Result<Self> {
        if members.len() < 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a group repository needs at least two members",
            ));
        }
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let bound = listener.local_addr()?;
        let shared = Arc::new(Shared::group(members));
        let stop = Arc::new(AtomicBool::new(false));
        spawn_acceptor(listener, Arc::clone(&shared), Arc::clone(&stop));
        Ok(Self {
            shared,
            addr: bound,
            stop,
        })
    }

    /// The address the server is bound to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Base URL for clients: `http://<addr>/` with a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Stored bytes for `path` (no leading `/`, e.g. `1.14.0/version.json`).
    pub fn store_get(&self, path: &str) -> Option<Vec<u8>> {
        lock(&self.shared.store).get(path).cloned()
    }

    /// Seed the store directly, bypassing all scenario behavior.
    pub fn insert(&self, path: &str, bytes: &[u8]) {
        lock(&self.shared.store).insert(path.to_owned(), bytes.to_vec());
    }

    /// Every request so far, in arrival order (including failures).
    pub fn requests(&self) -> Vec<ReqLog> {
        lock(&self.shared.log).clone()
    }

    /// Number of PUT requests for `path` that got a 2xx response.
    pub fn put_count(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|req| {
                req.method == "PUT"
                    && req.path == path
                    && matches!(req.outcome, Outcome::Status(200..=299))
            })
            .count()
    }

    /// Start serving synthesized (drifted) documents for `*/version.json` GETs.
    pub fn enable_drift(&self) {
        self.shared.drift.store(true, Ordering::Relaxed);
    }

    /// Return to store-backed document responses.
    pub fn disable_drift(&self) {
        self.shared.drift.store(false, Ordering::Relaxed);
    }
}

/// Accept connections until `stop` is set.
/// One thread per connection.
fn spawn_acceptor(listener: TcpListener, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    std::thread::spawn(move || loop {
        let Ok((conn, _)) = listener.accept() else {
            break;
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let conn_shared = Arc::clone(&shared);
        std::thread::spawn(move || scenario::serve(&conn_shared, conn));
    });
}

impl Drop for MockNexus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the blocking accept() so the listener thread exits and the port is released.
        let _ = TcpStream::connect(self.addr);
    }
}
