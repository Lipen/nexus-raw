//! Mock Nexus raw-storage server with a failure-scenario table.
//!
//! Implements just enough HTTP/1.1 (std only) to exercise the nexus-raw
//! transport contract: `GET`/`HEAD`/`PUT` with `Content-Length` or chunked
//! bodies, percent-encoded paths stored verbatim, one request per connection,
//! `Connection: close` on every response. GET honors resumable downloads: a
//! single open `Range: bytes=N-` is answered with `206` and `Content-Range`
//! (out-of-range starts get `416`); any other `Range` form is ignored.
//! A [`Scenario`] selects a failure mode (partial PUT bodies, connection
//! resets, slow links, drifted documents, flaky 503s, Basic-auth gating).
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

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use server::Drip;

/// Scenario names understood by the `mock-nexus` binary and by test harnesses.
pub const SCENARIOS: &[&str] = &[
    "atomic",
    "partial-put",
    "drop-connection",
    "freeze-upload",
    "sizeless",
    "slow",
    "foreign-marker",
    "markerless",
    "auth-401",
    "doc-drift",
    "flaky",
];

/// Failure scenario a [`MockNexus`] server simulates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scenario {
    /// Straight-through storage: every fully-read request is served normally.
    Atomic,
    /// The first PUT per path is cut off after `first_attempt_bytes` body
    /// bytes: the connection closes with no response and nothing is stored.
    /// Later PUT attempts on that path are read fully and stored.
    PartialPut { first_attempt_bytes: usize },
    /// The first request per path (any method) is read fully, then answered
    /// with a TCP reset. Later requests are served normally.
    DropConnection,
    /// PUT connections are held right after the head: the body is never
    /// read and no response is ever written, so the write side must detect
    /// the stall. GET/HEAD behave like [`Scenario::Atomic`].
    FreezeUpload,
    /// GET/HEAD answers for present objects carry no `Content-Length`:
    /// the proxy case. A client must refuse to treat such objects as
    /// absent (the digest comparison would be skipped). PUTs behave like
    /// [`Scenario::Atomic`].
    Sizeless,
    /// GET/HEAD bodies are written in `chunk_size` pieces, sleeping
    /// `chunk_delay_ms` between pieces. PUT bodies are read normally.
    Slow {
        chunk_delay_ms: u64,
        chunk_size: usize,
    },
    /// Stored `.sha256` markers get their 64-char digest replaced by 64 zeros
    /// (digest of a foreign object).
    /// Everything else is stored verbatim.
    ForeignMarker,
    /// `.sha256` markers are acknowledged (201 Created) but never stored.
    /// Non-marker bytes are stored normally.
    Markerless,
    /// Every request requires `Authorization: Basic base64(user:pass)`.
    /// Otherwise 401 with `WWW-Authenticate: Basic realm="nexus"`.
    /// Valid credentials behave like [`Scenario::Atomic`].
    Auth401 { user: String, pass: String },
    /// Behaves like [`Scenario::Atomic`] until [`MockNexus::enable_drift`].
    /// Afterwards every GET of a `*/version.json` path serves a synthesized
    /// version document with a ghost artifact.
    /// PUTs keep storing verbatim, and [`MockNexus::disable_drift`] restores store-backed responses.
    DocDrift,
    /// The first `first_failures` requests per path (any method) get
    /// 503 Service Unavailable.
    /// Later requests are served normally.
    Flaky { first_failures: u32 },
}

/// Outcome of one request recorded in the request log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A response with this status code was written.
    Status(u16),
    /// The connection was reset without a response.
    Reset,
    /// The request body ended after `bytes` bytes.
    /// No response was written.
    PartialRead { bytes: usize },
    /// The head was read, then the connection went silent: the body was
    /// never read and no response was ever written. The writer is expected
    /// to give up on its own (stall detection).
    Stalled,
}

/// One served request, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqLog {
    pub method: String,
    /// Raw request path: percent-encoding preserved, query stripped, no
    /// leading `/` (same key format as [`MockNexus::store_get`]).
    pub path: String,
    pub outcome: Outcome,
}

/// State shared by the accept loop and every connection thread.
#[derive(Debug)]
pub(crate) struct Shared {
    pub(crate) scenario: Scenario,
    pub(crate) store: Mutex<HashMap<String, Vec<u8>>>,
    pub(crate) log: Mutex<Vec<ReqLog>>,
    /// All-request counter per path (drop-connection, flaky gates).
    pub(crate) first_request: Mutex<HashMap<String, u32>>,
    /// PUT-only counter per path (partial-put gate).
    pub(crate) first_put: Mutex<HashMap<String, u32>>,
    pub(crate) drift: AtomicBool,
    /// Expected `Authorization` token when the scenario requires Basic auth.
    pub(crate) auth_b64: Option<String>,
    /// Slow-drip parameters for GET/HEAD bodies.
    pub(crate) drip: Option<Drip>,
    /// Success GET/HEAD answers hide `Content-Length` (the proxy case).
    pub(crate) sizeless: bool,
    /// Bytes served of the first PUT per path before cutting it (partial-put).
    pub(crate) partial_first_put: Option<usize>,
    /// Number of initial 503s per path (flaky).
    pub(crate) flaky_first: Option<u32>,
}

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
        let worker_shared = Arc::clone(&shared);
        let worker_stop = Arc::clone(&stop);
        std::thread::spawn(move || accept_loop(listener, worker_shared, worker_stop));

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
        // Unblock the blocking accept() so the listener thread exits and the
        // port is released.
        let _ = TcpStream::connect(self.addr);
    }
}

/// Accept connections until `stop` is set.
/// One thread per connection.
fn accept_loop(listener: TcpListener, shared: Arc<Shared>, stop: Arc<AtomicBool>) {
    loop {
        let conn = match listener.accept() {
            Ok((conn, _)) => conn,
            Err(_) => break,
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let conn_shared = Arc::clone(&shared);
        std::thread::spawn(move || scenario::serve(&conn_shared, conn));
    }
}

/// Lock a mutex, surviving poisoning: a panicking test thread must not take
/// unrelated assertions down with it.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{MockNexus, Outcome, Scenario};
    use std::io::{self, Read, Write};
    use std::net::{SocketAddr, TcpStream};

    /// Parsed response: (status, headers, body).
    type Response = (u16, Vec<(String, String)>, Vec<u8>);

    /// Open a connection and send a hand-built request.
    fn send(addr: SocketAddr, request: &[u8]) -> io::Result<TcpStream> {
        let mut stream = TcpStream::connect(addr)?;
        stream.write_all(request)?;
        Ok(stream)
    }

    /// Build a raw HTTP/1.1 request with an explicit Content-Length.
    fn request_bytes(method: &str, path: &str, body: &[u8], extra: &[(&str, &str)]) -> Vec<u8> {
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: mock\r\n");
        if !body.is_empty() || method == "PUT" {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        for (name, value) in extra {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    /// Read one full response.
    /// Returns (status, headers, body).
    fn read_response(stream: &mut TcpStream) -> io::Result<Response> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let head_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i;
            }
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated response",
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..head_end]);
        let mut lines = head.split("\r\n");
        let status_line = lines.next().unwrap_or_default();
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad status line"))?;
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();
        let content_length = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = buf[head_end + 4..].to_vec();
        while body.len() < content_length {
            // With Connection: close, EOF legitimately ends a bodyless (HEAD) response.
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
        body.truncate(content_length);
        Ok((status, headers, body))
    }

    /// Send a request and read the response back.
    fn exchange(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> io::Result<Response> {
        let mut stream = send(addr, &request_bytes(method, path, body, extra))?;
        read_response(&mut stream)
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn atomic_roundtrip_put_get_head() {
        let server = MockNexus::start(Scenario::Atomic).unwrap();
        assert_eq!(server.base_url(), format!("http://{}/", server.addr()));

        let (status, headers, body) =
            exchange(server.addr(), "PUT", "/1.14.0/sample.zip", b"PAYLOAD", &[]).unwrap();
        assert_eq!(status, 201);
        assert_eq!(header_value(&headers, "connection"), Some("close"));
        assert!(body.is_empty());

        let (status, headers, body) =
            exchange(server.addr(), "GET", "/1.14.0/sample.zip", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"PAYLOAD");
        assert_eq!(header_value(&headers, "content-length"), Some("7"));

        // Query strings are stripped before the store lookup.
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/sample.zip?x=1", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"PAYLOAD");

        // HEAD carries the same headers, including Content-Length, no body.
        let (status, headers, body) =
            exchange(server.addr(), "HEAD", "/1.14.0/sample.zip", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(header_value(&headers, "content-length"), Some("7"));
        assert!(body.is_empty());

        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/missing", b"", &[]).unwrap();
        assert_eq!(status, 404);
        assert_eq!(body, b"not found\n");

        assert_eq!(
            server.store_get("1.14.0/sample.zip").as_deref(),
            Some(b"PAYLOAD".as_slice())
        );
        assert_eq!(server.put_count("1.14.0/sample.zip"), 1);
    }

    #[test]
    fn partial_put_first_attempt_cut_then_stores() {
        let server = MockNexus::start(Scenario::PartialPut {
            first_attempt_bytes: 4,
        })
        .unwrap();

        // First PUT: server reads 4 body bytes, closes with no response.
        let mut stream = send(
            server.addr(),
            &request_bytes("PUT", "/1.14.0/big.zip", b"0123456789", &[]),
        )
        .unwrap();
        let mut buf = [0u8; 64];
        let probe = stream.read(&mut buf);
        // Reset (Err) and premature EOF (Ok(0)) both mean "no response arrived".
        assert!(
            matches!(probe, Err(_) | Ok(0)),
            "expected closed connection, got {probe:?}"
        );
        assert_eq!(server.store_get("1.14.0/big.zip"), None);

        // Second PUT is read fully and stored.
        let (status, _, body) =
            exchange(server.addr(), "PUT", "/1.14.0/big.zip", b"0123456789", &[]).unwrap();
        assert_eq!(status, 201);
        assert!(body.is_empty());
        assert_eq!(
            server.store_get("1.14.0/big.zip").as_deref(),
            Some(b"0123456789".as_slice())
        );

        // Log: cut attempt as PartialRead{4}, retry as 201.
        // Only the retry counts.
        let log = server.requests();
        assert_eq!(log[0].method, "PUT");
        assert_eq!(log[0].path, "1.14.0/big.zip");
        assert_eq!(log[0].outcome, Outcome::PartialRead { bytes: 4 });
        assert_eq!(log[1].outcome, Outcome::Status(201));
        assert_eq!(server.put_count("1.14.0/big.zip"), 1);
    }

    #[test]
    fn drop_connection_resets_first_request_per_path() {
        let server = MockNexus::start(Scenario::DropConnection).unwrap();
        server.insert("1.14.0/version.json", b"{}");

        let mut stream = send(
            server.addr(),
            &request_bytes("GET", "/1.14.0/version.json", b"", &[]),
        )
        .unwrap();
        let mut buf = [0u8; 64];
        let probe = stream.read(&mut buf);
        assert!(
            matches!(probe, Err(_) | Ok(0)),
            "expected reset, got {probe:?}"
        );
        assert_eq!(server.requests()[0].outcome, Outcome::Reset);

        // Later requests on the same path are served normally.
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/version.json", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn flaky_serves_503_for_first_k_requests() {
        let server = MockNexus::start(Scenario::Flaky { first_failures: 2 }).unwrap();
        server.insert("v1/a.bin", b"X");

        for _ in 0..2 {
            let (status, _, body) = exchange(server.addr(), "GET", "/v1/a.bin", b"", &[]).unwrap();
            assert_eq!(status, 503);
            assert_eq!(body, b"flaky\n");
        }
        let (status, _, body) = exchange(server.addr(), "GET", "/v1/a.bin", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"X");

        let outcomes: Vec<Outcome> = server
            .requests()
            .into_iter()
            .map(|req| req.outcome)
            .collect();
        assert_eq!(
            outcomes,
            vec![
                Outcome::Status(503),
                Outcome::Status(503),
                Outcome::Status(200)
            ]
        );
    }

    #[test]
    fn foreign_marker_zeroes_stored_digest() {
        let server = MockNexus::start(Scenario::ForeignMarker).unwrap();

        let marker = format!("{}  sample.zip\n", "a".repeat(64));
        let (status, _, _) = exchange(
            server.addr(),
            "PUT",
            "/1.14.0/sample.zip.sha256",
            marker.as_bytes(),
            &[],
        )
        .unwrap();
        assert_eq!(status, 201);
        assert_eq!(
            server.store_get("1.14.0/sample.zip.sha256").unwrap(),
            format!("{}  sample.zip\n", "0".repeat(64)).into_bytes()
        );

        // insert() bypasses the scenario: stored verbatim, served verbatim.
        server.insert("1.14.0/real.zip.sha256", marker.as_bytes());
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/real.zip.sha256", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, marker.as_bytes());
    }

    #[test]
    fn markerless_loses_marker_keeps_bytes() {
        let server = MockNexus::start(Scenario::Markerless).unwrap();

        let marker = format!("{}  sample.zip\n", "b".repeat(64));
        let (status, _, _) = exchange(
            server.addr(),
            "PUT",
            "/1.14.0/sample.zip.sha256",
            marker.as_bytes(),
            &[],
        )
        .unwrap();
        assert_eq!(status, 201);
        assert_eq!(server.store_get("1.14.0/sample.zip.sha256"), None);

        let (status, _, _) =
            exchange(server.addr(), "PUT", "/1.14.0/sample.zip", b"BYTES", &[]).unwrap();
        assert_eq!(status, 201);
        assert_eq!(
            server.store_get("1.14.0/sample.zip").as_deref(),
            Some(b"BYTES".as_slice())
        );

        // Both PUTs were acknowledged with 201.
        assert_eq!(server.put_count("1.14.0/sample.zip.sha256"), 1);
        assert_eq!(server.put_count("1.14.0/sample.zip"), 1);
    }

    #[test]
    fn doc_drift_synthesizes_between_enable_and_disable() {
        let server = MockNexus::start(Scenario::DocDrift).unwrap();
        let stored = br#"{"schema_version":1,"version":"1.14.0","artifacts":["real.zip"]}"#;
        server.insert("1.14.0/version.json", stored);

        // Before enable_drift: the store is served.
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/version.json", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, stored.to_vec());

        // After enable_drift: synthesized document with the version from the path.
        server.enable_drift();
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/version.json", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            body,
            br#"{"schema_version":1,"version":"1.14.0","artifacts":["ghost.zip"]}"#.to_vec()
        );

        // After disable_drift: back to the store.
        server.disable_drift();
        let (status, _, body) =
            exchange(server.addr(), "GET", "/1.14.0/version.json", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, stored.to_vec());
    }

    #[test]
    fn auth_401_gate() {
        let server = MockNexus::start(Scenario::Auth401 {
            user: "ci".to_owned(),
            pass: "secret".to_owned(),
        })
        .unwrap();
        server.insert("v1/a", b"X");

        // No credentials.
        let (status, headers, body) = exchange(server.addr(), "GET", "/v1/a", b"", &[]).unwrap();
        assert_eq!(status, 401);
        assert_eq!(body, b"auth required\n");
        assert_eq!(
            header_value(&headers, "www-authenticate"),
            Some("Basic realm=\"nexus\"")
        );

        // Wrong password.
        let (status, _, _) = exchange(
            server.addr(),
            "GET",
            "/v1/a",
            b"",
            &[("Authorization", "Basic Y2k6d3Jvbmc=")], // ci:wrong
        )
        .unwrap();
        assert_eq!(status, 401);

        // Valid credentials: normal behavior.
        let (status, _, body) = exchange(
            server.addr(),
            "GET",
            "/v1/a",
            b"",
            &[("Authorization", "Basic Y2k6c2VjcmV0")], // ci:secret
        )
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(body, b"X");
    }

    #[test]
    fn slow_get_roundtrip() {
        let server = MockNexus::start(Scenario::Slow {
            chunk_delay_ms: 1,
            chunk_size: 3,
        })
        .unwrap();
        let body: Vec<u8> = (0..20u32).map(|i| b'a' + (i % 26) as u8).collect();
        server.insert("v/slow.bin", &body);

        let (status, headers, got) =
            exchange(server.addr(), "GET", "/v/slow.bin", b"", &[]).unwrap();
        assert_eq!(status, 200);
        assert_eq!(header_value(&headers, "content-length"), Some("20"));
        assert_eq!(got, body);
    }

    #[test]
    fn get_range_serves_suffix_body_and_rejects_out_of_bounds() {
        let server = MockNexus::start(Scenario::Atomic).unwrap();

        // Store the object through the wire, as a resuming client would.
        let (status, _, _) =
            exchange(server.addr(), "PUT", "/1.14.0/blob.bin", b"0123456789", &[]).unwrap();
        assert_eq!(status, 201);

        // Resume from byte 5: 206 with a Content-Range and the suffix body.
        let (status, headers, body) = exchange(
            server.addr(),
            "GET",
            "/1.14.0/blob.bin",
            b"",
            &[("Range", "bytes=5-")],
        )
        .unwrap();
        assert_eq!(status, 206);
        assert_eq!(
            header_value(&headers, "content-range"),
            Some("bytes 5-9/10")
        );
        assert_eq!(header_value(&headers, "content-length"), Some("5"));
        assert_eq!(body, b"56789");
        // The range hit is recorded as 206.
        assert_eq!(server.requests()[1].outcome, Outcome::Status(206));

        // Start at the end: nothing to serve, 416 with the object total.
        let (status, headers, _) = exchange(
            server.addr(),
            "GET",
            "/1.14.0/blob.bin",
            b"",
            &[("Range", "bytes=10-")],
        )
        .unwrap();
        assert_eq!(status, 416);
        assert_eq!(header_value(&headers, "content-range"), Some("bytes */10"));

        // A range on a missing object is still a plain 404.
        let (status, _, body) = exchange(
            server.addr(),
            "GET",
            "/1.14.0/missing",
            b"",
            &[("Range", "bytes=5-")],
        )
        .unwrap();
        assert_eq!(status, 404);
        assert_eq!(body, b"not found\n");
    }
}
