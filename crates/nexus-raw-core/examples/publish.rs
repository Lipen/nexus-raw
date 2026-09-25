//! Publish a version directory against the bundled mock server, then fetch it back.
//!
//! Self-contained: the example starts its own mock on an ephemeral port, so it
//! runs with zero setup.
//!
//! ```bash
//! cargo run -p nexus-raw-core --example publish
//! ```

use std::time::Duration;

use nexus_raw_core::config::Config;
use nexus_raw_core::model::claim::Claim;
use nexus_raw_core::model::digest::Digest;
use nexus_raw_core::model::name::ArtifactName;
use nexus_raw_core::model::sibling;
use nexus_raw_core::{Event, Nxr};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), nexus_raw_core::Error> {
    // A throwaway server with the default (correct) behavior.
    let mock = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic).expect("mock starts");

    // Stage a version directory: claim + bytes + marker.
    let dir = tempfile::tempdir().expect("temp dir");
    let content = b"example artifact";
    std::fs::write(dir.path().join("artifact.bin"), content).expect("write bytes");
    let marker = sibling::format_line("artifact.bin", &Digest::of_bytes(content));
    std::fs::write(dir.path().join("artifact.bin.sha256"), marker).expect("write marker");
    std::fs::write(
        dir.path().join("claim.json"),
        Claim {
            claim_version: 1,
            version: "1.0.0".into(),
            artifacts: vec![ArtifactName::parse("artifact.bin").expect("valid name")],
        }
        .to_bytes(),
    )
    .expect("write claim");

    // The event channel: one line per progress event, here just printed.
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("{}", event.to_json());
        }
    });

    let nxr = Nxr::new(
        Config {
            base: mock.base_url(),
            workers: 4,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(5),
            stall_timeout: Duration::from_secs(30),
            tls_insecure: false,
            auth: None,
        },
        tx,
    )?;

    // Publish: claim first (drift check), then bytes and marker per name.
    let claim = Claim::from_slice(&std::fs::read(dir.path().join("claim.json")).expect("claim"))?;
    let summary = nxr.up(dir.path(), &claim, None).await?;
    println!("published: {summary:?}");

    // Consume into a fresh directory; the second run would transfer nothing.
    let target = tempfile::tempdir().expect("temp dir");
    let summary = nxr.down(target.path(), &claim, None, None).await?;
    println!("fetched: {summary:?}");

    // Dropping the facade closes the event channel, so the printer finishes.
    drop(nxr);
    printer.await.expect("printer joins");
    Ok(())
}
