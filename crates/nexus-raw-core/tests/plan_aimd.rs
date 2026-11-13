//! Conformance: a `Retry-After` during the plan phase shrinks the same worker
//! budget the transfer then runs under.
//!
//! The probe that sees the 429 is the run's first remote request. Before the
//! budget moved before the plan, `up` attached a fresh full budget after the
//! probes, so a throttled plan left the transfer at the configured maximum.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use nexus_raw_core::{Config, Nxr};
use tokio::sync::mpsc;

fn config(base: String) -> Config {
    Config {
        base,
        ca_bundle: None,
        workers: 8,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(30),
        tls_insecure: false,
        auth: None,
        user_agent: None,
    }
}

fn read_head(stream: &mut std::net::TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1024, "oversized request head");
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

fn content_length(head: &str) -> usize {
    head.lines()
        .find_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0)
}

fn respond(stream: &mut std::net::TcpStream, status: &str, headers: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();
}

#[tokio::test]
async fn throttled_plan_shrinks_the_transfer_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        // 1: the plan's HEAD, throttled with a server-mandated pause.
        let (mut stream, _) = listener.accept().unwrap();
        let head = read_head(&mut stream);
        tx.send(format!("first: {}", head.lines().next().unwrap_or("")))
            .unwrap();
        respond(
            &mut stream,
            "429 Too Many Requests",
            "Retry-After: 1\r\n",
            &[],
        );

        // 2: the retried HEAD finds the name absent.
        let (mut stream, _) = listener.accept().unwrap();
        let head = read_head(&mut stream);
        tx.send(format!("retry: {}", head.lines().next().unwrap_or("")))
            .unwrap();
        respond(&mut stream, "404 Not Found", "", &[]);

        // 3 and 4: the bytes and the marker.
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let head = read_head(&mut stream);
            let mut body = vec![0u8; content_length(&head)];
            stream.read_exact(&mut body).unwrap();
            tx.send(format!("put: {}", head.lines().next().unwrap_or("")))
                .unwrap();
            respond(&mut stream, "201 Created", "", &[]);
        }
    });

    let base = format!("http://127.0.0.1:{port}/1.0.0/");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.bin"), b"payload").unwrap();
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(base.clone()), ev_tx).unwrap();

    let summary = nxr.up(dir.path(), None, true, None).await.unwrap();
    assert_eq!(summary.uploaded, 1);
    assert_eq!(summary.failed, Vec::<String>::new());

    let (final_limit, min_limit) = nxr.last_run_limit();
    assert_eq!(
        min_limit, 4,
        "the plan-phase 429 must halve the configured 8, got {min_limit}"
    );
    assert!(
        final_limit <= 8,
        "the budget never exceeds the configured maximum"
    );

    drop(_ev_rx);
    let seen: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
    assert_eq!(
        seen.len(),
        4,
        "expected probe, retry, bytes and marker, got {seen:?}"
    );
    assert!(seen[0].starts_with("first: HEAD"), "{seen:?}");
    assert!(seen[1].starts_with("retry: HEAD"), "{seen:?}");
    assert!(seen[2].starts_with("put: PUT"), "{seen:?}");
    assert!(seen[3].starts_with("put: PUT"), "{seen:?}");
}
