//! Conformance tests driven through the public library API.

use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};

use crate::{MockNexus, Outcome, Scenario, Spec};

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
        let _ = write!(head, "Content-Length: {}\r\n", body.len());
    }
    for (name, value) in extra {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// Read one full response.
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

    let (status, _, body) = exchange(server.addr(), "GET", "/1.14.0/missing", b"", &[]).unwrap();
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

/// atomic DELETE: an existing object goes away with 204, an absent one answers 404.
#[test]
fn atomic_delete_removes_then_404s() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    server.insert("1.14.0/a.zip", b"PAYLOAD");

    let (status, _, body) = exchange(server.addr(), "DELETE", "/1.14.0/a.zip", b"", &[]).unwrap();
    assert_eq!(status, 204);
    assert!(body.is_empty());
    assert_eq!(server.store_get("1.14.0/a.zip"), None);

    // The second DELETE is a normal 404: deletion stays idempotent.
    let (status, _, body) = exchange(server.addr(), "DELETE", "/1.14.0/a.zip", b"", &[]).unwrap();
    assert_eq!(status, 404);
    assert_eq!(body, b"not found\n");
}

/// readonly: every DELETE is a 403 and nothing leaves the store, reads included.
#[test]
fn readonly_refuses_every_delete() {
    let server = MockNexus::start(Scenario::ReadOnly).unwrap();
    server.insert("1.14.0/a.zip", b"PAYLOAD");

    let (status, _, body) = exchange(server.addr(), "DELETE", "/1.14.0/a.zip", b"", &[]).unwrap();
    assert_eq!(status, 403);
    assert_eq!(body, b"read-only\n");
    assert_eq!(
        server.store_get("1.14.0/a.zip").as_deref(),
        Some(b"PAYLOAD".as_slice()),
        "a refused delete must not touch the store"
    );

    // Reads behave like atomic.
    let (status, _, body) = exchange(server.addr(), "GET", "/1.14.0/a.zip", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"PAYLOAD");
}

/// search-lag: the store-backed page reports the size of the stored bytes, the
/// field the real answer carries.
#[test]
fn search_lag_page_reports_the_stored_size() {
    let server = MockNexus::start(Scenario::SearchLag { lag_ms: 0 }).unwrap();
    // One PUT arms the catch-up deadline and stores the object.
    let (status, _, _) = exchange(
        server.addr(),
        "PUT",
        "/repository/raw-main/1.0.0/a.bin",
        b"PAYLOAD",
        &[],
    )
    .unwrap();
    assert_eq!(status, 201);

    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/search/assets?repository=raw-main",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains(r#""fileSize":7"#),
        "the page reports the stored size: {text}"
    );
}

/// search-lag: the store-backed page carries the artifact digest from the stored
/// marker, and a marker row reports no digest rather than a placeholder.
#[test]
fn search_lag_page_reports_the_marker_digest() {
    let server = MockNexus::start(Scenario::SearchLag { lag_ms: 0 }).unwrap();
    let digest = "ab".repeat(32);
    let (status, _, _) = exchange(
        server.addr(),
        "PUT",
        "/repository/raw-main/1.0.0/a.bin",
        b"PAYLOAD",
        &[],
    )
    .unwrap();
    assert_eq!(status, 201);
    let marker = format!("{digest}  a.bin\n");
    let (status, _, _) = exchange(
        server.addr(),
        "PUT",
        "/repository/raw-main/1.0.0/a.bin.sha256",
        marker.as_bytes(),
        &[],
    )
    .unwrap();
    assert_eq!(status, 201);

    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/search/assets?repository=raw-main",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 200);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains(&format!(
            r#""fileSize":7,"checksum":{{"sha256":"{digest}"}}"#
        )),
        "the page echoes the stored digest: {text}"
    );
    assert!(
        text.contains(&format!(
            r#""path":"/1.0.0/a.bin.sha256","id":"raw-main:1.0.0/a.bin.sha256","repository":"raw-main","format":"raw","fileSize":{}"#,
            marker.len()
        )),
        "the marker row is listed without invented facts: {text}"
    );
    assert!(
        !text.contains("0000000000000000000000000000000000000000000000000000000000000000"),
        "no placeholder digest: {text}"
    );
}

/// static-host: reads and ranges, every write refused with 405 and `Allow`, no service plane.
#[test]
fn static_host_serves_reads_and_refuses_writes() {
    let server = MockNexus::start(Scenario::StaticHost).unwrap();
    server.insert("a.bin", b"PAYLOAD");

    let (status, headers, body) = exchange(server.addr(), "GET", "/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"PAYLOAD");
    assert_eq!(header_value(&headers, "content-length"), Some("7"));

    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/a.bin",
        b"",
        &[("Range", "bytes=1-3")],
    )
    .unwrap();
    assert_eq!(status, 206);
    assert_eq!(body, b"AYL");

    for method in ["PUT", "DELETE"] {
        let (status, headers, body) =
            exchange(server.addr(), method, "/a.bin", b"NEW", &[]).unwrap();
        assert_eq!(status, 405, "{method}");
        assert_eq!(
            header_value(&headers, "allow"),
            Some("GET,HEAD"),
            "{method}: the group wire shape"
        );
        assert!(body.is_empty(), "{method}");
    }
    assert_eq!(
        server.store_get("a.bin").as_deref(),
        Some(b"PAYLOAD".as_slice()),
        "a refused write changes nothing"
    );

    for path in [
        "/service/rest/v1/repositories",
        "/service/rest/v1/search/assets?repository=raw-main",
        "/service/rest/v1/status",
    ] {
        let (status, _, _) = exchange(server.addr(), "GET", path, b"", &[]).unwrap();
        assert_eq!(status, 404, "{path}: a static host has no service plane");
    }
}

/// search-400: a repository-scoped search for an unknown repository is refused with 400, a served repository keeps the store answer.
#[test]
fn search_unknown_repo_answers_400_known_repos_pass_through() {
    let server = MockNexus::start(Scenario::Search400).unwrap();
    server.insert(
        "service/rest/v1/search/assets",
        br#"{"continuationToken":null,"items":[{"path":"a.txt"}]}"#,
    );

    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/search/assets?repository=raw-ghost",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 400);
    assert!(find(&body, b"unknown repository").is_some(), "{body:?}");

    // A served repository keeps the store answer, and a search without a repository parameter is left alone.
    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/search/assets?repository=raw-main",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 200);
    assert!(find(&body, b"a.txt").is_some(), "{body:?}");

    let (status, _, _) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/search/assets",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 200);
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
    let server = MockNexus::start(Scenario::Flaky { first_failures: 3 }).unwrap();
    server.insert("v1/a.bin", b"X");

    for _ in 0..2 {
        let (status, _, body) = exchange(server.addr(), "GET", "/v1/a.bin", b"", &[]).unwrap();
        assert_eq!(status, 503);
        assert_eq!(body, b"flaky\n");
    }
    // The third 503 lands on a HEAD: Content-Length mirrors the body, no body bytes ride the wire.
    let (status, headers, body) = exchange(server.addr(), "HEAD", "/v1/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 503);
    assert_eq!(header_value(&headers, "content-length"), Some("6"));
    assert!(body.is_empty(), "a HEAD response carries no body: {body:?}");
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
            Outcome::Status(503),
            Outcome::Status(200)
        ]
    );
}

#[test]
fn retry_after_503_serves_one_pause_then_atomic() {
    let server = MockNexus::start(Scenario::RetryAfter503 {
        retry_after_secs: 7,
    })
    .unwrap();
    server.insert("v1/a.bin", b"X");

    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 503);
    assert_eq!(header_value(&headers, "retry-after"), Some("7"));
    assert_eq!(body, b"retry-after-503\n");

    // The pause is per path: a fresh path pays its own 503, the same path is served.
    let (status, _, body) = exchange(server.addr(), "GET", "/v1/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"X");
    let (status, headers, _) = exchange(server.addr(), "PUT", "/v1/b.bin", b"Y", &[]).unwrap();
    assert_eq!(status, 503);
    assert_eq!(header_value(&headers, "retry-after"), Some("7"));
    let (status, _, _) = exchange(server.addr(), "PUT", "/v1/b.bin", b"Y", &[]).unwrap();
    assert_eq!(status, 201);
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

    // The same verdict on a HEAD: Content-Length stays, no body bytes ride the wire.
    let (status, headers, body) = exchange(server.addr(), "HEAD", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 401);
    assert_eq!(header_value(&headers, "content-length"), Some("14"));
    assert!(body.is_empty(), "a HEAD response carries no body: {body:?}");

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

/// auth-403: everything but the valid Basic token answers 403 `forbidden`, on any method.
/// Valid credentials behave like atomic.
#[test]
fn auth_403_gate() {
    let server = MockNexus::start(Scenario::Auth403 {
        user: "ci".to_owned(),
        pass: "secret".to_owned(),
    })
    .unwrap();
    server.insert("v1/a", b"X");

    // No credentials: 403 on a GET.
    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 403);
    assert_eq!(body, b"forbidden\n");
    assert_eq!(header_value(&headers, "www-authenticate"), None);

    // A HEAD carries the same verdict with no body bytes (RFC 9110 §9.3.2).
    let (status, headers, body) = exchange(server.addr(), "HEAD", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 403);
    assert_eq!(header_value(&headers, "content-length"), Some("10"));
    assert!(body.is_empty(), "a HEAD response carries no body: {body:?}");

    // Wrong password is still just a 403, on PUT too: the seeded bytes are never overwritten.
    let (status, _, body) = exchange(
        server.addr(),
        "PUT",
        "/v1/a",
        b"BYTES",
        &[("Authorization", "Basic Y2k6d3Jvbmc=")], // ci:wrong
    )
    .unwrap();
    assert_eq!(status, 403);
    assert_eq!(body, b"forbidden\n");
    assert_eq!(server.store_get("v1/a").as_deref(), Some(b"X".as_slice()));

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

/// rate-limit: the first N requests per path answer 429 with `Retry-After` and an empty body, then the path serves like atomic.
#[test]
fn rate_limit_serves_429_then_atomic() {
    let server = MockNexus::start(Scenario::RateLimit {
        first_429s: 2,
        retry_after_secs: 7,
    })
    .unwrap();
    server.insert("v1/a", b"X");

    for _ in 0..2 {
        let (status, headers, body) = exchange(server.addr(), "GET", "/v1/a", b"", &[]).unwrap();
        assert_eq!(status, 429);
        assert_eq!(header_value(&headers, "retry-after"), Some("7"));
        assert!(body.is_empty());
    }
    let (status, _, body) = exchange(server.addr(), "GET", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"X");

    // The gate counts per path, any method: a first PUT on a fresh path is throttled too, the retry burns the second 429 and the third attempt lands.
    let (status, headers, _) = exchange(server.addr(), "PUT", "/v1/b", b"Y", &[]).unwrap();
    assert_eq!(status, 429);
    assert_eq!(header_value(&headers, "retry-after"), Some("7"));
    assert_eq!(server.store_get("v1/b"), None);

    let (status, _, _) = exchange(server.addr(), "PUT", "/v1/b", b"Y", &[]).unwrap();
    assert_eq!(status, 429);
    assert_eq!(server.store_get("v1/b"), None);

    let (status, _, _) = exchange(server.addr(), "PUT", "/v1/b", b"Y", &[]).unwrap();
    assert_eq!(status, 201);
    assert_eq!(server.store_get("v1/b").as_deref(), Some(b"Y".as_slice()));

    let outcomes: Vec<Outcome> = server
        .requests()
        .into_iter()
        .map(|req| req.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            Outcome::Status(429),
            Outcome::Status(429),
            Outcome::Status(200),
            Outcome::Status(429),
            Outcome::Status(429),
            Outcome::Status(201)
        ]
    );
}

/// redirect: GET and HEAD answer 301 with `Location` and an empty body, writes stay atomic.
#[test]
fn redirect_answers_301_on_reads_only() {
    let server = MockNexus::start(Scenario::Redirect {
        location_path: "/repository/raw/moved".to_owned(),
    })
    .unwrap();
    server.insert("v1/a", b"X");

    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 301);
    assert_eq!(
        header_value(&headers, "location"),
        Some("/repository/raw/moved")
    );
    assert_eq!(header_value(&headers, "content-length"), Some("0"));
    assert!(body.is_empty());

    let (status, _, body) = exchange(server.addr(), "HEAD", "/v1/a", b"", &[]).unwrap();
    assert_eq!(status, 301);
    assert!(body.is_empty());

    // Writes ignore the redirect and behave like atomic.
    let (status, _, _) = exchange(server.addr(), "PUT", "/v1/b", b"Y", &[]).unwrap();
    assert_eq!(status, 201);
    assert_eq!(server.store_get("v1/b").as_deref(), Some(b"Y".as_slice()));
    let (status, _, _) = exchange(server.addr(), "DELETE", "/v1/b", b"", &[]).unwrap();
    assert_eq!(status, 204);
    assert_eq!(server.store_get("v1/b"), None);
}

/// cut-body with an honest length: the first GET breaks after honest headers and a short body, a fresh GET serves the object whole.
#[test]
fn cut_body_first_get_truncated_then_honest() {
    let server = MockNexus::start(Scenario::CutBody {
        after_bytes: 4,
        fake_length: false,
    })
    .unwrap();
    server.insert("v1/big.bin", b"0123456789");

    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/big.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    // Honest Content-Length, fewer bytes than promised: a short read for the client.
    assert_eq!(header_value(&headers, "content-length"), Some("10"));
    assert_eq!(body, b"0123");

    // The retry serves the object whole.
    let (status, _, body) = exchange(server.addr(), "GET", "/v1/big.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"0123456789");
}

/// cut-body with a lying length: the first GET promises `total + 1024` and breaks after `after_bytes`.
/// The retry is honest.
#[test]
fn cut_body_fake_length_lies_high() {
    let server = MockNexus::start(Scenario::CutBody {
        after_bytes: 4,
        fake_length: true,
    })
    .unwrap();
    server.insert("v1/big.bin", b"0123456789");

    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/big.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(header_value(&headers, "content-length"), Some("1034"));
    assert_eq!(body, b"0123");

    let (status, headers, body) = exchange(server.addr(), "GET", "/v1/big.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(header_value(&headers, "content-length"), Some("10"));
    assert_eq!(body, b"0123456789");
}

/// A head past the 64 KiB bound is answered with 400, not a silent hang-up.
#[test]
fn oversized_head_is_answered_with_400() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    let long_header = "X".repeat(70 * 1024);
    let request = request_bytes("GET", "/v1/a", b"", &[("X-Long", &long_header)]);
    let mut stream = send(server.addr(), &request).unwrap();
    let (status, _, body) = read_response(&mut stream).unwrap();
    assert_eq!(status, 400);
    assert_eq!(body, b"bad request\n");
    assert_eq!(server.requests()[0].outcome, Outcome::Status(400));
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

    let (status, headers, got) = exchange(server.addr(), "GET", "/v/slow.bin", b"", &[]).unwrap();
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

// ---------------------------------------------------------------- group

#[test]
fn group_forwards_reads_in_order_and_refuses_writes() {
    let first = MockNexus::start(Scenario::Atomic).unwrap();
    let second = MockNexus::start(Scenario::Atomic).unwrap();
    let group = MockNexus::start_group(&[&first, &second]).unwrap();

    // The same key in both members with different bytes: the first member's copy wins.
    first.insert("repository/raw/1.0.0/x.bin", b"first");
    second.insert("repository/raw/1.0.0/x.bin", b"second");

    let (status, headers, body) =
        exchange(group.addr(), "GET", "/repository/raw/1.0.0/x.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"first");
    assert_eq!(header_value(&headers, "content-length"), Some("5"));

    // Nobody holds the key: the plain single-instance miss.
    let (status, _, body) = exchange(
        group.addr(),
        "GET",
        "/repository/raw/1.0.0/missing",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 404);
    assert_eq!(body, b"not found\n");

    // Writes are refused by the group itself, in the real Nexus wire shape.
    let (status, headers, body) = exchange(
        group.addr(),
        "PUT",
        "/repository/raw/1.0.0/x.bin",
        b"payload",
        &[],
    )
    .unwrap();
    assert_eq!(status, 405);
    assert_eq!(header_value(&headers, "allow"), Some("GET,HEAD"));
    assert_eq!(header_value(&headers, "content-length"), Some("0"));
    assert!(body.is_empty(), "real Nexus sends no 405 body: {body:?}");
    let (status, headers, _) = exchange(
        group.addr(),
        "DELETE",
        "/repository/raw/1.0.0/x.bin",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 405);
    assert_eq!(header_value(&headers, "allow"), Some("GET,HEAD"));

    // No member ever saw a write, and both stores are intact.
    for member in [&first, &second] {
        assert!(
            member
                .requests()
                .iter()
                .all(|r| r.method != "PUT" && r.method != "DELETE"),
            "a group write must not reach a member: {:?}",
            member.requests()
        );
    }
    assert_eq!(
        first.store_get("repository/raw/1.0.0/x.bin").as_deref(),
        Some(b"first".as_slice())
    );

    // The group's own log records the client-visible answers in order.
    let statuses: Vec<u16> = group
        .requests()
        .iter()
        .filter_map(|r| match r.outcome {
            Outcome::Status(s) => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(statuses, [200, 404, 405, 405]);
    assert_eq!(group.put_count("repository/raw/1.0.0/x.bin"), 0);
}

#[test]
fn group_walks_past_a_failing_member() {
    let flaky = MockNexus::start(Scenario::Flaky {
        first_failures: 999,
    })
    .unwrap();
    let atomic = MockNexus::start(Scenario::Atomic).unwrap();
    let group = MockNexus::start_group(&[&flaky, &atomic]).unwrap();
    atomic.insert("repository/raw/1.0.0/x.bin", b"payload");

    // The flaky member's 503 is skipped: atomic's bytes serve the read, and the client sees nothing of the refusal.
    let (status, _, body) =
        exchange(group.addr(), "GET", "/repository/raw/1.0.0/x.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"payload");

    let refused = flaky
        .requests()
        .iter()
        .any(|r| matches!(r.outcome, Outcome::Status(503)));
    let served = atomic
        .requests()
        .iter()
        .any(|r| matches!(r.outcome, Outcome::Status(200)));
    assert!(
        refused && served,
        "the walk must reach both members: {:?} then {:?}",
        flaky.requests(),
        atomic.requests()
    );
}

#[test]
fn group_needs_at_least_two_members() {
    let solo = MockNexus::start(Scenario::Atomic).unwrap();
    let err = MockNexus::start_group(&[&solo]).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn group_head_miss_sends_no_body() {
    let first = MockNexus::start(Scenario::Atomic).unwrap();
    let second = MockNexus::start(Scenario::Atomic).unwrap();
    let group = MockNexus::start_group(&[&first, &second]).unwrap();

    // A HEAD miss must not put a body on the wire (RFC 9110 §9.3.2), while the
    // Content-Length still mirrors the single-repo miss.
    let (status, headers, body) = exchange(
        group.addr(),
        "HEAD",
        "/repository/raw/1.0.0/missing",
        b"",
        &[],
    )
    .unwrap();
    assert_eq!(status, 404);
    assert_eq!(header_value(&headers, "content-length"), Some("10"));
    assert!(body.is_empty(), "a HEAD response carries no body: {body:?}");
}

#[test]
fn get_closed_range_serves_the_exact_span() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    let content: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    server.insert("big.bin", &content);

    let (status, headers, body) = exchange(
        server.addr(),
        "GET",
        "/big.bin",
        b"",
        &[("Range", "bytes=25000-49999")],
    )
    .unwrap();
    assert_eq!(status, 206);
    assert_eq!(
        header_value(&headers, "content-range"),
        Some("bytes 25000-49999/100000")
    );
    assert_eq!(body, &content[25000..50000]);

    // A closed range ending past the object's end clips to the stored bytes.
    let (status, headers, body) = exchange(
        server.addr(),
        "GET",
        "/big.bin",
        b"",
        &[("Range", "bytes=99990-999999")],
    )
    .unwrap();
    assert_eq!(status, 206);
    assert_eq!(
        header_value(&headers, "content-range"),
        Some("bytes 99990-99999/100000")
    );
    assert_eq!(body, &content[99990..]);
}

#[test]
fn spec_layers_compose_and_report() {
    // Two layers at once: an auth gate AND a flaky gate, which no single preset expresses.
    let server =
        MockNexus::start_spec(Spec::atomic().with_auth("ci", "secret").with_flaky(1)).unwrap();
    assert_eq!(server.scenario_name(), "spec");

    // The auth gate fires first: an unauthenticated request is a 401 challenge, not a 503.
    let (status, _, _) = exchange(server.addr(), "GET", "/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 401, "the auth gate outranks the flaky gate");
    // Authenticated: the flaky gate consumes its one 503, then the miss is a plain 404.
    let auth = [("Authorization", "Basic Y2k6c2VjcmV0")]; // base64("ci:secret")
    let (status, _, _) = exchange(server.addr(), "GET", "/a.bin", b"", &auth).unwrap();
    assert_eq!(
        status, 503,
        "the flaky layer fires on the first authenticated request"
    );
    let (status, _, _) = exchange(server.addr(), "GET", "/a.bin", b"", &auth).unwrap();
    assert_eq!(status, 404, "past the flaky budget the miss is a plain 404");
}

#[test]
fn preset_scenario_names_stay_canonical() {
    let server = MockNexus::start(Scenario::AuditLitter).unwrap();
    assert_eq!(server.scenario_name(), "audit-litter");
}

/// Walk a generated search through its continuation tokens: every `path`, in page order.
fn search_paths(addr: SocketAddr, query: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let target = match &token {
            None => format!("/service/rest/v1/search/assets?repository=raw-main{query}"),
            Some(token) => format!(
                "/service/rest/v1/search/assets?repository=raw-main{query}&continuationToken={token}"
            ),
        };
        let (status, _, body) = exchange(addr, "GET", &target, b"", &[]).unwrap();
        assert_eq!(status, 200, "the generated search answers");
        let page: serde_json::Value = serde_json::from_slice(&body).expect("a JSON search page");
        if let Some(items) = page["items"].as_array() {
            paths.extend(
                items
                    .iter()
                    .filter_map(|item| item["path"].as_str().map(str::to_owned)),
            );
        }
        match page["continuationToken"].as_str() {
            Some(next) => token = Some(next.to_owned()),
            None => return paths,
        }
    }
}

/// assets-prefix-q: the search `q` is the pinned server's loose path substring.
/// A directory value with the trailing slash cuts to that directory, and still drags a copy of
/// it under another parent in.
/// A value without the slash also drags the name-continuing sibling in, and percent-encoding is
/// decoded before the match.
#[test]
fn assets_prefix_q_matches_loosely_like_the_pinned_server() {
    let server = MockNexus::start(Scenario::AssetsPrefixQ).unwrap();

    let scoped = search_paths(server.addr(), "&q=v1.1.0-dir%2F");
    assert_eq!(
        scoped.len(),
        500,
        "300 directory rows plus 200 decoy rows, the copy under another parent"
    );
    assert!(
        scoped.iter().any(|p| p.starts_with("/backup/v1.1.0-dir/")),
        "the loose match drags the copy in: the client filter is what drops it"
    );
    assert!(
        !scoped.iter().any(|p| p.starts_with("/v1.1.0-dir2/")),
        "the trailing slash keeps the name-continuing sibling out of the cut"
    );
    assert!(
        !scoped.iter().any(|p| p.starts_with("/v1.0.0-bulk/")),
        "the bulk stays out of a directory cut"
    );

    let unslashed = search_paths(server.addr(), "&q=v1.1.0-dir");
    assert_eq!(
        unslashed.len(),
        504,
        "the sibling rides a value without the trailing slash"
    );
    assert!(
        unslashed.iter().any(|p| p.starts_with("/v1.1.0-dir2/")),
        "without the slash the sibling is dragged in"
    );

    let sibling_only = search_paths(server.addr(), "&q=v1.1.0-dir2%2F");
    assert_eq!(
        sibling_only.len(),
        4,
        "the sibling directory cuts on its own"
    );
}

/// The query value is percent-decoded before the match, and a malformed escape stays verbatim.
#[test]
fn query_percent_decoding_keeps_malformed_escapes() {
    assert_eq!(
        crate::support::percent_decode("v1.1.0-dir%2F"),
        "v1.1.0-dir/"
    );
    assert_eq!(crate::support::percent_decode("pl%2Bus"), "pl+us");
    assert_eq!(
        crate::support::percent_decode("name%20with%20space"),
        "name with space"
    );
    assert_eq!(crate::support::percent_decode("bad%2"), "bad%2");
    assert_eq!(crate::support::percent_decode("plain"), "plain");
}

/// An inverted range is invalid: the header is ignored, the whole object is served, and the fixture stays healthy.
#[test]
fn inverted_range_is_ignored() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    let (status, _, _) =
        exchange(server.addr(), "PUT", "/1.14.0/blob.bin", b"0123456789", &[]).unwrap();
    assert_eq!(status, 201);

    let (status, headers, body) = exchange(
        server.addr(),
        "GET",
        "/1.14.0/blob.bin",
        b"",
        &[("Range", "bytes=5-3")],
    )
    .unwrap();
    assert_eq!(status, 200, "an inverted range is ignored, not a panic");
    assert_eq!(header_value(&headers, "content-range"), None);
    assert_eq!(body, b"0123456789");

    // The malformed request did not poison the server: the next one answers normally.
    let (status, _, body) = exchange(server.addr(), "GET", "/1.14.0/blob.bin", b"", &[]).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"0123456789");
}

/// A declared length larger than the body cap is refused before any allocation.
#[test]
fn huge_content_length_is_refused_without_a_panic() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    let mut stream = send(
        server.addr(),
        b"PUT /huge.bin HTTP/1.1\r\nHost: mock\r\nContent-Length: 18446744073709551615\r\n\r\n",
    )
    .unwrap();
    let (status, _, _) = read_response(&mut stream).unwrap();
    assert_eq!(status, 400);

    // The fixture survives the hostile header and answers the next request.
    let (status, _, _) = exchange(server.addr(), "GET", "/", b"", &[]).unwrap();
    assert!(status == 200 || status == 404, "got {status}");
}

/// A chunked body whose size line crosses the cap is refused, not reserved.
#[test]
fn huge_chunked_body_is_refused() {
    let server = MockNexus::start(Scenario::Atomic).unwrap();
    let mut stream = send(
        server.addr(),
        b"PUT /huge.bin HTTP/1.1\r\nHost: mock\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFFF\r\n",
    )
    .unwrap();
    let (status, _, _) = read_response(&mut stream).unwrap();
    assert_eq!(status, 400);
}

/// The redirect knob is read from the spec, so `Spec::with_redirect` and `--config` both work.
#[test]
fn spec_redirect_is_honored() {
    let server = MockNexus::start_spec(Spec::atomic().with_redirect("/moved")).unwrap();
    let (status, headers, body) = exchange(server.addr(), "GET", "/1.0.0/a.bin", b"", &[]).unwrap();
    assert_eq!(status, 301);
    assert_eq!(header_value(&headers, "location"), Some("/moved"));
    assert!(body.is_empty());

    // Writes stay atomic under a read redirect.
    let (status, _, _) = exchange(server.addr(), "PUT", "/1.0.0/a.bin", b"x", &[]).unwrap();
    assert_eq!(status, 201);
}

/// The scoped repositories document adds `raw-secret` and never duplicates a seeded name.
#[test]
fn scoped_repo_collection_lists_each_name_once() {
    let server = MockNexus::start(Scenario::RepoCollectionScoped {
        user: "ci".into(),
        pass: "secret".into(),
    })
    .unwrap();
    let auth = format!("Basic {}", crate::base64::encode(b"ci:secret"));
    let (status, _, body) = exchange(
        server.addr(),
        "GET",
        "/service/rest/v1/repositories",
        b"",
        &[("Authorization", &auth)],
    )
    .unwrap();
    assert_eq!(status, 200);
    let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = doc
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["name"].as_str())
        .collect();
    assert_eq!(
        names.iter().filter(|name| **name == "raw-main").count(),
        1,
        "raw-main appears once: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|name| **name == "raw-secret").count(),
        1,
        "raw-secret appears once: {names:?}"
    );
}
