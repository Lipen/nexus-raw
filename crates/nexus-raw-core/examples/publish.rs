//! Publish a version directory against the bundled mock server, then fetch it back.
//!
//! Self-contained: the example starts its own mock on an ephemeral port, so it
//! runs with zero setup.
//!
//! ```bash
//! cargo run -p nexus-raw-core --example publish
//! ```

use std::sync::Arc;
use std::time::Duration;

use mock_nexus::{MockNexus, Scenario};
use nexus_raw_core::{ArtifactName, Config, Enumeration, Event, Nxr};

// The current-thread flavor keeps the example buildable without the `rt-multi-thread` feature
// (the workspace tokio does not enable it; the CLI and the TUI add it for their own binaries).
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The mock: a tiny std-only Nexus with a failure-scenario table.
    let mock = Arc::new(MockNexus::start(Scenario::Atomic)?);
    let base = format!("http://{}/1.4.0/", mock.addr());

    // A local version directory: up generates the marker for it by default.
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("artifact.bin"), b"payload-bytes-16")?;

    // The facade: NDJSON events flow through the channel.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("{}", event.to_json());
        }
    });
    let nxr = nxr_for(&base, tx)?;

    // Up: markers are written by default, so the remote copy is complete.
    let summary = nxr.up(dir.path(), None, true, None).await?;
    println!("published: {summary:?}");

    // Down into a second directory; enumeration comes from explicit names.
    let out = tempfile::tempdir()?;
    let summary = nxr
        .down(
            out.path(),
            Enumeration::Names(vec![ArtifactName::parse("artifact.bin")?]),
            false,
        )
        .await?;
    println!("fetched: {summary:?}");

    // Dropping the facade closes the event channel, so the printer finishes.
    drop(nxr);
    drop(mock);
    printer.await.expect("printer joins");
    Ok(())
}

fn nxr_for(
    base: &str,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<Nxr, nexus_raw_core::Error> {
    let cfg = Config {
        base: base.to_owned(),
        tls_insecure: false,
        workers: 4,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(5),
        stall_timeout: Duration::from_secs(5),
        auth: None,
    };
    Nxr::new(cfg, tx)
}
