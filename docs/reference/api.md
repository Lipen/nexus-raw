# Rust API

`nexus-raw-core` is the protocol without a UI: the same operations the CLI exposes, as a small async API around one facade.

## Add the dependency

```toml
[dependencies]
nexus-raw-core = { path = "crates/nexus-raw-core" }   # inside this workspace
# or, once published:
# nexus-raw-core = "0.1"
```

## The facade

```rust
use std::time::Duration;
use nexus_raw_core::{Config, Nxr, Event};

#[tokio::main]
async fn main() -> Result<(), nexus_raw_core::Error> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // Drain events while the operation runs (or after, from the buffered queue).
    let renderer = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            println!("{}", ev.to_json());
        }
    });

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

    // Dropping the facade closes the event channel, so the renderer finishes.
    drop(nxr);
    renderer.await.unwrap();
    Ok(())
}
```

!!! note "The facade owns the sender"

    `Nxr::new` takes the event sender and emits through it for the facade's lifetime — keep `nxr` alive until you are done receiving events.

## Operations

| Method | Returns | Mirrors |
|:-------|:--------|:--------|
| `read_remote_claim(&version)` | `Option<Claim>` | the claim behind `ls --version` |
| `resolve_pointer(&pointer)` | `String` | `down --pointer` resolution |
| `diff(&dir, &claim)` | `Vec<Action>` | `nxr diff` |
| `up(&dir, &claim, plan)` | `Summary` | `nxr up` |
| `down(&dir, &claim, only, plan)` | `Summary` | `nxr down` |
| `verify(&dir, &claim)` | `Summary` | `nxr verify` |
| `point(&pointer, &version, if_newer)` | `PointOutcome` | `nxr point` |
| `remote_state(&version)` | per-name `RemoteStatus` | `nxr ls --version` |
| `ls_versions()` | `Vec<String>` | `nxr ls` (experimental) |

`plan: Option<Vec<Action>>` accepts a precomputed diff — useful to print the plan, then execute exactly it.

## Events

`Event` is the progress stream: `Plan`, `ArtifactStarted`, `ArtifactBytes` (coalesced to one per 200 ms per name), `ArtifactDone`, `Retrying`, `Summary`.
`Event::to_json()` yields the same shapes the CLI prints, so an embedding UI and the NDJSON output cannot diverge.

## Errors

`nexus_raw_core::Error` is the whole taxonomy.
`Error::exit_code()` maps it to the CLI's exit classes.
`Verdict` (diff refusals) converts into `Error` with `From`, so a refused diff and a refused transfer look identical to a caller.

## Guarantees

- No protocol logic outside the crate: the CLI is flags and rendering only.
- Credentials never appear in `Event`s, error messages or `Display` impls.
- The diff classification, marker format and write order follow [the protocol](protocol.md) exactly.
