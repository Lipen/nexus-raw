//! The wire headers a native client sends: `User-Agent` names the tool and its version.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;

use nexus_raw_core::{Config, Nxr};

/// One connection, one request head: the answer is a 200 with two bytes, and the
/// returned handle carries the request line and the headers as received.
fn serve_once() -> (
    String,
    std::net::SocketAddr,
    std::thread::JoinHandle<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).expect("read line") == 0 {
                break;
            }
            if line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .expect("write answer");
        head
    });
    (format!("http://{addr}/obj.bin"), addr, handle)
}

fn config(url: &str, user_agent: Option<String>) -> Config {
    Config {
        base: url.to_owned(),
        tls_insecure: false,
        ca_bundle: None,
        workers: 1,
        retry_attempts: 1,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(5),
        auth: None,
        user_agent,
    }
}

#[tokio::test]
async fn the_default_user_agent_names_the_tool_and_its_version() {
    let (url, _addr, server) = serve_once();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&url, None), tx).expect("client");
    let dir = tempfile::tempdir().expect("temp");
    nxr.get(&url, Some(dir.path().join("obj.bin")), false, true)
        .await
        .expect("get");
    drop(nxr);
    let head = server.join().expect("server").to_ascii_lowercase();
    let expected = format!("user-agent: nxr/{}\r\n", env!("CARGO_PKG_VERSION"));
    assert!(
        head.contains(&expected),
        "the request carries {expected:?}: {head}"
    );
}

#[tokio::test]
async fn an_embedder_names_itself_in_the_user_agent() {
    let (url, _addr, server) = serve_once();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let nxr = Nxr::new(config(&url, Some("pouch/1.2".to_owned())), tx).expect("client");
    let dir = tempfile::tempdir().expect("temp");
    nxr.get(&url, Some(dir.path().join("obj.bin")), false, true)
        .await
        .expect("get");
    drop(nxr);
    let head = server.join().expect("server").to_ascii_lowercase();
    assert!(
        head.contains("user-agent: pouch/1.2\r\n"),
        "the configured identity wins: {head}"
    );
}
