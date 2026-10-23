//! The publish-and-consume loop through the `nexus-raw-core` library API, in one process.
//!
//! Without arguments the demo spawns the repo's mock server on :8090.
//! Pass a base URL to run against a real repository instead:
//!
//! ```text
//! cargo run --manifest-path examples/rust/Cargo.toml -- https://nexus.example.com/repository/demo/
//! ```

use nexus_raw_core::{Config, Nxr};
use std::io::{BufRead, BufReader};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = match std::env::args().nth(1) {
        Some(url) => url,
        None => spawn_mock()?,
    };

    // The producer side: a version directory with two artifacts.
    // A manifest.json of your own is an ordinary file: it rides `up` like any name.
    let dist = std::env::current_dir()?.join("demo-dist");
    let _ = std::fs::remove_dir_all(&dist);
    std::fs::create_dir_all(dist.join("bom"))?;
    std::fs::write(dist.join("app.bin"), vec![7u8; 4096])?;
    std::fs::write(dist.join("bom/data.json"), br#"{"kind": "bom"}"#)?;

    let cfg = Config {
        base: format!("{repo}1.0.0/"),
        tls_insecure: false,
        ca_bundle: None,
        workers: 8,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(15),
        stall_timeout: Duration::from_secs(30),
        auth: None,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<nexus_raw_core::Event>();
    let printer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            println!("  {}", ev.to_json());
        }
    });
    let nxr = Nxr::new(cfg, tx)?;

    let up = nxr.up(&dist, None, true, None).await?;
    println!(
        "up: uploaded {}, skipped {}",
        up.uploaded, up.skipped
    );

    // Name the version so consumers never hard-code it.
    nxr.channel_set(&format!("{repo}latest"), "1.0.0", true).await?;
    let version = nxr.channel_get(&format!("{repo}latest")).await?.unwrap();
    println!("channel latest -> {version}");

    // The consumer side: a fresh process would re-point Config at the resolved version URL.
    // Enumeration is explicit: the search listing (`--ls` on the CLI) or names you pass yourself.
    let vendor = std::env::current_dir()?.join("demo-vendor");
    let _ = std::fs::remove_dir_all(&vendor);
    let down = nxr
        .down(
            &vendor,
            nexus_raw_core::Enumeration::Names(
                ["app.bin", "bom/data.json"]
                    .iter()
                    .map(|n| nexus_raw_core::ArtifactName::parse(n))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            false,
        )
        .await?;
    println!(
        "down: downloaded {}, skipped {}",
        down.downloaded, down.skipped
    );

    let check = nxr.verify(&vendor, None).await?;
    println!("verify: ok, {} names verified", check.skipped + down.downloaded);
    printer.abort();
    Ok(())
}

/// Spawn the repo's mock server on an ephemeral port and return its demo repository base URL.
/// The child is leaked on purpose: it serves until the demo process exits.
fn spawn_mock() -> Result<String, Box<dyn std::error::Error>> {
    // The demo runs from the repo root (just) or from examples/rust (cargo): try both, and let the environment override.
    let candidates: Vec<String> = if let Ok(bin) = std::env::var("MOCK_NEXUS_BIN") {
        vec![bin]
    } else {
        vec![
            "target/debug/mock-nexus".to_owned(),
            "../../target/debug/mock-nexus".to_owned(),
        ]
    };
    let mut child = None;
    for bin in &candidates {
        if let Ok(c) = std::process::Command::new(bin)
            .arg("atomic")
            // Port 0 = an ephemeral port: a busy fixed port would collide with whatever else lives on this machine.
            .arg("--port")
            .arg("0")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            child = Some(c);
            break;
        }
    }
    let mut child =
        child.ok_or("cannot spawn mock-nexus (build it: cargo build -p mock-nexus)")?;
    let stdout = child.stdout.take().expect("stdout was piped");
    // The banner is written at startup: a blocking read is fine here.
    let first_line = BufReader::new(stdout).lines().next();
    match first_line.transpose()? {
        Some(line) if line.starts_with("listening http://") => {
            let mut base = line["listening ".len()..].to_owned();
            if !base.ends_with('/') {
                base.push('/');
            }
            Ok(base)
        }
        other => Err(format!("unexpected mock banner: {other:?}").into()),
    }
}
