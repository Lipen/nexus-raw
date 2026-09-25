# nexus-raw-core

The protocol core of nexus-raw: claim, sha-sibling markers, symmetric diff, resumable upload and download against a Sonatype Nexus raw repository.
The `nxr` CLI and the npm wrapper are thin shells over this crate — all protocol logic lives here.

## Usage

```rust
use std::time::Duration;
use nexus_raw_core::{Config, Nxr, Event};

#[tokio::main]
async fn main() -> Result<(), nexus_raw_core::Error> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // Spawn a task to render events, or just buffer them.
    let nxr = Nxr::new(
        Config {
            base: "https://nexus.example.com/repository/raw-main/".into(),
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            tls_insecure: false,
            auth: None,
        },
        tx,
    )?;

    let claim = nexus_raw_core::Claim::read("dist/1.4.0/claim.json".as_ref())?;
    nxr.up("dist/1.4.0".as_ref(), &claim, None).await?;
    drop(nxr); // closing the sender ends the event stream in `rx`
    let _ = rx;
    Ok(())
}
```

## Guarantees

- Completion is bytes plus a `<name>.sha256` marker, digest matching, `sha256sum -c` format.
- The symmetric diff drives both directions, so a repeat command finishes an interrupted transfer.
- Divergent complete artifacts are refused, never overwritten.
- Claims are immutable, drift is a hard error.
- Credentials never appear in events, errors or logs.

## More

- A runnable example against the bundled mock: `cargo run -p nexus-raw-core --example publish`
- The crate docs carry the protocol summary: `cargo doc -p nexus-raw-core --open`
- The user-facing documentation site lives one level up: [docs/](../../docs/)
