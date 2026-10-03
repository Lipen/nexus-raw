//! Mock Nexus raw-storage server with a failure-scenario table.
//!
//! Implements just enough HTTP/1.1 (std only) to exercise the nexus-raw transport contract.
//! `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, `DELETE` with 204/404.
//! Percent-encoded paths stored verbatim.
//! One request per connection, `Connection: close` on every response.
//! GET honors resumable downloads: a single open `Range: bytes=N-` is answered with `206` and `Content-Range`, out-of-range starts get `416`.
//! Any other `Range` form is ignored.
//! A [`Scenario`] selects a failure mode: partial PUT bodies, connection resets, slow links, drifted documents, flaky 503s, Basic-auth gating.
//!
//! Rust conformance tests use the library API directly.
//! The `mock-nexus` binary exposes the same scenarios to shell- and Python-driven tests:
//!
//! ```text
//! let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic)?;
//! let url = format!("{}1.14.0/version.json", server.base_url());
//! ```

mod base64;
mod scenario;
mod server;
mod store;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use scenario::{Scenario, SCENARIOS};
pub use store::{Outcome, ReqLog};

use server::Drip;
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

        let auth_b64 = match &scenario {
            Scenario::Auth401 { user, pass } => {
                Some(base64::encode(format!("{user}:{pass}").as_bytes()))
            }
            _ => None,
        };
        let drip = match &scenario {
            Scenario::Slow {
                chunk_delay_ms,
                chunk_size,
            } => Some(Drip {
                delay_ms: *chunk_delay_ms,
                chunk_size: *chunk_size,
            }),
            _ => None,
        };
        let sizeless = matches!(&scenario, Scenario::Sizeless);
        let (partial_first_put, flaky_first) = match &scenario {
            Scenario::PartialPut {
                first_attempt_bytes,
            } => (Some(*first_attempt_bytes), None),
            Scenario::Flaky { first_failures } => (None, Some(*first_failures)),
            _ => (None, None),
        };

        let shared = Arc::new(Shared {
            scenario,
            store: Mutex::new(HashMap::new()),
            log: Mutex::new(Vec::new()),
            first_request: Mutex::new(HashMap::new()),
            first_put: Mutex::new(HashMap::new()),
            drift: AtomicBool::new(false),
            auth_b64,
            drip,
            sizeless,
            partial_first_put,
            flaky_first,
        });
        let stop = Arc::new(AtomicBool::new(false));
        {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            // Accept connections until `stop` is set.
            // One thread per connection.
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
                    && matches!(req.outcome, Outcome::Status(200..=300))
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

impl Drop for MockNexus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock the blocking accept() so the listener thread exits and the port is released.
        let _ = TcpStream::connect(self.addr);
    }
}
