//! An external consumer's view of `nexus-raw-core`: a standalone crate that
//! pulls the library by path (a crates.io version when published), owns its
//! own workspace and runs the whole publish-and-consume loop in one process.
//!
//! Without arguments the demo spawns the repo's mock server on :8090.
//! Pass a base URL to run against a real repository instead:
//!
//! ```text
//! cargo run --manifest-path examples/rust/Cargo.toml -- https://nexus.example.com/repository/demo/
//! ```

use nexus_raw_core::{ArtifactName, Config, Nxr};
use std::io::{BufRead, BufReader};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = match std::env::args().nth(1) {
        Some(url) => url,
        None => spawn_mock()?,
    };

    // The producer side: a version directory with two artifacts and the
    // manifest that names them.
    let dist = std::env::current_dir()?.join("demo-dist");
    std::fs::create_dir_all(dist.join("bom"))?;
    std::fs::write(dist.join("app.bin"), vec![7u8; 4096])?;
    std::fs::write(dist.join("bom/manifest.json"), br#"{"artifacts": ["app.bin"]}"#)?;
    // In a real pipeline the manifest sits at the version root and lists
    // exactly what consumers may fetch.
    std::fs::copy(dist.join("bom/manifest.json"), dist.join("manifest.json"))?;

    let cfg = Config {
        base: format!("{repo}1.0.0/"),
        tls_insecure: false,
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

    let claim = ArtifactName::parse("manifest.json")?;
    let up = nxr.up(&dist, None, true, Some(claim), None).await?;
    println!(
        "up: uploaded {}, skipped {}",
        up.uploaded, up.skipped
    );

    // Name the version so consumers never hard-code it.
    nxr.channel_set(&format!("{repo}latest"), "1.0.0", true).await?;
    let version = nxr.channel_get(&format!("{repo}latest")).await?.unwrap();
    println!("channel latest -> {version}");

    // The consumer side: a fresh process would re-point Config at the
    // resolved version URL and read manifest.json from there.
    let vendor = std::env::current_dir()?.join("demo-vendor");
    let _ = std::fs::remove_dir_all(&vendor);
    let manifest = std::fs::read(dist.join("manifest.json"))?;
    let enum_src = nexus_raw_core::Manifest::from_slice(&manifest)?;
    let down = nxr.down(&vendor, nexus_raw_core::Enumeration::Manifest(enum_src), false, None).await?;
    println!(
        "down: downloaded {}, skipped {}",
        down.downloaded, down.skipped
    );

    let check = nxr.verify(&vendor, None).await?;
    println!("verify: ok, {} names verified", check.skipped + down.downloaded);
    printer.abort();
    Ok(())
}

/// Spawn the repo's mock server on an ephemeral port and return its demo
/// repository base URL. The child is leaked on purpose: it serves until the
/// demo process exits.
fn spawn_mock() -> Result<String, Box<dyn std::error::Error>> {
    // The demo runs from the repo root (just) or from examples/rust (cargo):
    // try both, and let the environment override.
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
            // Port 0 = an ephemeral port: a busy fixed port would collide
            // with whatever else lives on this machine.
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
