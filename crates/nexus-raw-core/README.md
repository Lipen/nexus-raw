# nexus-raw-core

The core of nexus-raw: curl for a Sonatype Nexus raw repository.
Four public layers (transport + primitives, transfer, layout helpers, the `Nxr` facade), with the CLI as a thin shell that carries no protocol logic.

## Layers

| Layer | Modules | Content |
|:------|:--------|:--------|
| L0 | `transport`, `primitive` | retries, backoff, stall detection, TLS, auth and the `get`/`put`/`head`/`sha` commands |
| L1 | `sync` | directory up/down, symmetric diff, sha-sibling markers, Range-resume |
| L2 | `layout` | channels (token files with any name), manifests, search listings |
| L3 | the CLI crate | doctor, hints, human and NDJSON rendering |

Layers never import upward.

## Usage

```rust
use std::time::Duration;
use nexus_raw_core::{Config, Enumeration, Nxr, Event};

#[tokio::main]
async fn main() -> Result<(), nexus_raw_core::Error> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    let nxr = Nxr::new(
        Config {
            base: "https://nexus.example.com/repository/raw-main/1.4.0/".into(),
            tls_insecure: false,
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            auth: Some("Basic Y2ktYm90OnRva2Vu".into()),
        },
        tx,
    )?;

    // Verified upload of a directory: markers generated and written by default.
    nxr.up("dist/1.4.0".as_ref(), None, true, None).await?;

    // Verified download, enumerated by the version's manifest.
    // Resume is the default: `fresh = false` continues the part files of an interrupted run.
    let manifest = nxr
        .manifest_from("https://nexus.example.com/repository/raw-main/1.4.0/manifest.json")
        .await?;
    nxr.down("vendor/1.4.0".as_ref(), Enumeration::Manifest(manifest), false)
        .await?;

    let _ = rx;
    drop(nxr); // dropping the facade closes the event stream
    Ok(())
}
```

`Config` is one invocation's settings, there is no config file.
`auth` is the ready `Authorization` header, resolved by the caller from `-u user:pass`, `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD`.

## Guarantees

- Completion is bytes plus a `<name>.sha256` sibling, digest matching, `sha256sum -c` format.
- `up` writes markers by default and generates missing local siblings, `--no-sha` opts out.
- `down` requires an enumeration source (a manifest, explicit names or the search API) and refuses to guess.
- The symmetric diff drives both directions, so a repeat command finishes an interrupted transfer.
- Divergent complete artifacts are refused, never overwritten.
- Credentials never appear in events, errors or logs.

## More

- The crate docs carry the layer map and the store shape: `cargo doc -p nexus-raw-core --open`
- The CLI that wraps this crate: [nexus-raw](../nexus-raw/)
- The user-facing documentation site lives one level up: [docs/](../../docs/)
