//! The [`MockNexus`] handle: start, stop, store access and the request log.

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::scenario::{self, Scenario};
use crate::store::{lock, Outcome, ReqLog, Shared};

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
        let shared = Arc::new(Shared::hosted(scenario, format!("http://{bound}/")));
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
        match listener.accept() {
            Ok((conn, _)) => {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let conn_shared = Arc::clone(&shared);
                std::thread::spawn(move || scenario::serve(&conn_shared, conn));
            }
            // A burst of short-lived connections lets some abort between the
            // handshake and this accept: the name is reusable, take the next one.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            Err(_) => break,
        }
    });
}

impl Drop for MockNexus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the blocking accept() so the listener thread exits and the port is released.
        let _ = TcpStream::connect(self.addr);
    }
}
