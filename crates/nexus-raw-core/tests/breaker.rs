//! Conformance: a server that stops answering stops the run.
//!
//! Each probe spends its own retry budget before it fails, so without the
//! run-level breaker a dead server grinds every name in the enumeration.
//! After `RUN_FAILURE_LIMIT` transport failures in a row the client must stop
//! opening connections, and the run must end with a transport error instead of
//! walking the rest of the list.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexus_raw_core::{ArtifactName, Config, Enumeration, Nxr};
use tokio::sync::mpsc;

fn config(base: String) -> Config {
    Config {
        base,
        ca_bundle: None,
        workers: 8,
        // One attempt per request: the breaker counts request outcomes, and the
        // test must not spend the backoff ladder on every probe.
        retry_attempts: 1,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(300),
        tls_insecure: false,
        auth: None,
        user_agent: None,
    }
}

#[tokio::test]
async fn a_dead_server_stops_the_run() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let accepts_thread = accepts.clone();
    let stop_thread = stop.clone();
    std::thread::spawn(move || loop {
        if stop_thread.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                accepts_thread.fetch_add(1, Ordering::Relaxed);
                // Drop at once: the client sees the connection end before the
                // response, which is a transport failure.
                drop(stream);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(_) => break,
        }
    });

    let base = format!("http://127.0.0.1:{port}/1.0.0/");
    let dir = tempfile::tempdir().unwrap();
    let names: Vec<ArtifactName> = (0..200)
        .map(|i| ArtifactName::parse(&format!("f{i:03}.bin")).unwrap())
        .collect();
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(base), ev_tx).unwrap();

    let started = Instant::now();
    let result = nxr.down(dir.path(), Enumeration::Names(names), false).await;
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);

    let accepted = accepts.load(Ordering::Relaxed);
    let error = result.expect_err("a dead server must fail the run");
    assert!(
        matches!(error, nexus_raw_core::Error::Transport { .. }),
        "expected a transport verdict, got {error:?}"
    );
    assert!(
        accepted >= 8,
        "the run never reached the server: {accepted} connections"
    );
    assert!(
        accepted <= 48,
        "the breaker did not stop the walk: {accepted} connections for 200 names"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the run ground through the list: {elapsed:?}"
    );
}
