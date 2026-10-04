//! Conformance: a zstd-encoded GET decodes transparently, and the digest still describes the original bytes.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use nexus_raw_core::sync::down::part_path;
use nexus_raw_core::{ArtifactName, Config, Digest, Enumeration, Nxr};
use sha2::Digest as _;
use sha2::Sha256;
use tokio::sync::mpsc;

fn config(base: String) -> Config {
    Config {
        base,
        workers: 1,
        retry_attempts: 2,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(30),
        tls_insecure: false,
        auth: None,
    }
}

/// Read one request head, through the blank line that ends it.
fn read_head(stream: &mut std::net::TcpStream) -> String {
    read_head_dbg(stream)
}

#[allow(dead_code)]
fn read_head_dbg(stream: &mut std::net::TcpStream) -> String {
    let s = read_head_impl(stream);
    eprintln!("[listener] got: {}", s.lines().next().unwrap_or(""));
    s
}

fn read_head_impl(stream: &mut std::net::TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 16 * 1024, "oversized request head");
        stream.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
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
async fn zstd_body_decodes_and_digest_matches_original() {
    let payload: Vec<u8> = (0..256 * 1024).map(|i| (i / 97) as u8).collect();
    let frame = zstd::encode_all(&payload[..], 3).unwrap();
    let digest_hex = Sha256::digest(&payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx_flags, mut rx_flags) = mpsc::unbounded_channel::<bool>();
    let digest_hex_in_thread = digest_hex.clone();
    std::thread::spawn(move || {
        // The artifact request: the client asks for zstd, the server answers with it.
        let (mut stream, _) = listener.accept().unwrap();
        let head = read_head(&mut stream).to_ascii_lowercase();
        tx_flags
            .send(head.contains("accept-encoding:") && head.contains("zstd"))
            .unwrap();
        respond(
            &mut stream,
            "200 OK",
            "Content-Type: application/octet-stream\r\nContent-Encoding: zstd\r\n",
            &frame,
        );

        // The sibling request: plain bytes, the header may be present, nothing decodes.
        let (mut stream, _) = listener.accept().unwrap();
        read_head(&mut stream);
        respond(
            &mut stream,
            "200 OK",
            "Content-Type: text/plain\r\n",
            format!("{digest_hex_in_thread}  big.bin\n").as_bytes(),
        );
    });

    let base = format!("http://127.0.0.1:{port}/1.0.0/");
    let dir = tempfile::tempdir().unwrap();
    let out: PathBuf = dir.path().join("big.bin");
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(base.clone()), ev_tx).unwrap();

    let outcome = nxr
        .get(&format!("{base}big.bin"), Some(out.clone()), false)
        .await
        .unwrap();
    assert_eq!(outcome.size, payload.len() as u64, "decoded size");

    let expected = Digest::from_hex(&digest_hex).unwrap();
    assert_eq!(
        outcome.digest,
        Some(expected.clone()),
        "digest over decoded bytes"
    );

    let sibling_path = dir.path().join("big.bin.sha256");
    nxr.get(
        &format!("{base}big.bin.sha256"),
        Some(sibling_path.clone()),
        false,
    )
    .await
    .unwrap();
    let served = std::fs::read_to_string(&sibling_path).unwrap();
    let hex_part = served.split_whitespace().next().unwrap();
    assert_eq!(
        Digest::from_hex(hex_part).unwrap(),
        expected,
        "sibling agrees with the decoded content"
    );

    drop(_ev_rx);
    assert!(
        rx_flags.try_recv().unwrap(),
        "the artifact request did not ask for zstd"
    );
}

#[tokio::test]
async fn resume_request_pins_identity_encoding() {
    let payload: Vec<u8> = (0..128 * 1024).map(|i| (i / 89) as u8).collect();
    let prefix_len = 4096_usize;
    let digest_hex = Sha256::digest(&payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx_flags, mut rx_flags) = mpsc::unbounded_channel::<bool>();
    let digest_hex_in_thread = digest_hex.clone();
    let payload_in_thread = payload.clone();
    let total_len = payload.len();
    std::thread::spawn(move || {
        // down() probes with HEAD, verifies the sibling, then resumes the bytes.
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let head = read_head(&mut stream);
            let first = head.lines().next().unwrap_or("").to_string();
            if first.starts_with("HEAD") {
                // Existence probe: headers only, a HEAD response never carries a body.
                let out = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {total_len}\r\nConnection: close\r\n\r\n");
                stream.write_all(out.as_bytes()).unwrap();
                stream.flush().unwrap();
            } else if first.contains(".sha256") {
                respond(
                    &mut stream,
                    "200 OK",
                    "Content-Type: text/plain\r\n",
                    format!("{digest_hex_in_thread}  big.bin\n").as_bytes(),
                );
            } else {
                let lower = head.to_ascii_lowercase();
                tx_flags
                    .send(
                        lower.contains("range: bytes=4096-")
                            && lower.contains("accept-encoding: identity"),
                    )
                    .unwrap();
                let rest = &payload_in_thread[prefix_len..];
                respond(
                    &mut stream,
                    "206 Partial Content",
                    &format!(
                        "Content-Type: application/octet-stream\r\nContent-Range: bytes {prefix_len}-{}/{}\r\n",
                        total_len - 1,
                        total_len
                    ),
                    rest,
                );
            }
        }
    });

    let base = format!("http://127.0.0.1:{port}/1.0.0/");
    let dir = tempfile::tempdir().unwrap();
    let name = ArtifactName::parse("big.bin").unwrap();
    let part = part_path(dir.path(), &name);
    std::fs::write(&part, &payload[..prefix_len]).unwrap();

    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(config(base.clone()), ev_tx).unwrap();
    nxr.down(dir.path(), Enumeration::Names(vec![name]), false)
        .await
        .unwrap();

    let final_file = dir.path().join("big.bin");
    assert_eq!(
        std::fs::read(&final_file).unwrap(),
        payload,
        "the resumed file is byte-equal to the original"
    );

    drop(_ev_rx);
    assert!(
        rx_flags.try_recv().unwrap(),
        "the resume request did not pin accept-encoding: identity"
    );
}
