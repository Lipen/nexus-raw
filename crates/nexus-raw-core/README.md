# nexus-raw-core

The protocol core of [nexus-raw](https://github.com/Lipen/nexus-raw): verified, resumable directory transfers against Sonatype Nexus raw storage, as a library.

It suits tools that move artifacts themselves, such as a release service, an internal CLI or a CI helper.
The `nxr` binary, the `nxr-tui` browser, the `nxr-mount` FUSE view and the Node bindings are built on this crate.

- Completion is bytes plus a `<name>.sha256` marker in `sha256sum -c` format.
- Both directions run the same symmetric diff, so a repeat call finishes an interrupted transfer instead of starting over.
- A divergent complete artifact is refused, never overwritten.
- Every error carries `exit_code()` (`0` ok, `1` data, `2` misuse, `3` transport) and a `hint()` line.
- Credentials never appear in events, errors or logs.

## Install

```bash
cargo add nexus-raw-core
```

The native transfer surface needs a multi-thread Tokio runtime.
On `wasm32` the filesystem-walking part is compiled out and the read/write subset remains, which is what the browser sandbox uses.

## Usage

```rust
use std::time::Duration;

use nexus_raw_core::{ArtifactName, Config, Enumeration, Event, Nxr};

#[tokio::main]
async fn main() -> Result<(), nexus_raw_core::Error> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("{event:?}");
        }
    });

    // One invocation, one Config: no config file, no profiles.
    let nxr = Nxr::new(
        Config {
            base: "https://nexus.example.com/repository/raw-main/1.4.0/".into(),
            tls_insecure: false,
            ca_bundle: None,
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            // The ready `Authorization` header, resolved by the caller from
            // `-u`, `NXR_AUTH`, or `NXR_USERNAME` + `NXR_PASSWORD`.
            auth: None,
        },
        tx,
    )?;

    // Publish a directory: markers are generated and written by default.
    let summary = nxr.up("dist/1.4.0".as_ref(), None, true, None).await?;
    println!("uploaded {}", summary.uploaded);

    // Fetch it back: `fresh = false` resumes the part files of a broken run.
    let names = vec![ArtifactName::parse("app-1.4.0.zip")?];
    let summary = nxr
        .down("vendor/1.4.0".as_ref(), Enumeration::Names(names), false)
        .await?;
    println!("downloaded {}", summary.downloaded);

    drop(nxr); // dropping the facade closes the event stream
    Ok(())
}
```

`auth` is the finished header value, not a login: it is `Option<String>` and renders as `***` in `Debug`.
The `Enumeration` you pass to `down`, `diff`, `rm` and `mirror` is the name source: `Names`, `Search` (the search API, best effort), or either of them wrapped by `.with_prefixes(&["bom/"])`.

## What is in it

| Surface | Calls |
|:---|:---|
| primitives | `get`, `get_segmented`, `get_range`, `put`, `put_bytes`, `head`, `sha`, `sibling_digest`, `read_range` |
| transfers | `scan`, `diff`, `delta`, `up`, `up_if_changed`, `down`, `down_with_segments`, `clone_plan`, `clone_to`, `mirror`, `mirror_plan` |
| layout | `channel_get`, `channel_set`, `ls_entries`, `ls_assets`, `enumerate` |
| integrity | `audit`, `audit_filtered`, `verify` |
| deletion | `rm`, `rm_plan`, `point_clear` |
| server API | `service_repos`, `service_status`, `service_repo`, `service_assets`, `service_eula` |

Every plan call returns the actions instead of writing, which is how `--plan` and dry runs are built.

## Guarantees

- `up` writes markers by default and generates the missing local siblings.
  The bytes are written first and the marker of the same name after them.
- `down` requires an enumeration source: an empty name list is an error rather than an implicit "everything".
- Names are relative paths of `[A-Za-z0-9._-]` segments.
  Only the `.sha256` and `.part` suffixes and a `.nxr`-prefixed segment are reserved.
- An interrupted download stays in a hidden `.nxr-part-<hash>` and never reaches its final path half-written.
  Resume is a `Range` request from the byte already on disk.
- A channel is any token file.
  `--if-forward` imposes dotted-numeric order and nothing else.

<details>
<summary>The four layers</summary>

Dependencies point one way, and no layer imports upward.

| Layer | Modules | Content |
|:------|:--------|:--------|
| L0 | `transport`, `primitive` | retries, backoff, stall detection, TLS, auth, and `get`/`put`/`head`/`sha` |
| L1 | `sync` | directory up/down, symmetric diff, sha-sibling markers, `Range` resume |
| L2 | `layout` | channels (token files with any name) and search listings |
| L3 | the `Nxr` facade and `Event` | one entry point, one event stream, errors with exit codes and hints |

The CLI lives outside this crate and carries no protocol logic.

</details>

## Documentation

- The crate docs (`cargo doc -p nexus-raw-core --open`) carry the layer map and the store shape.
- The wire protocol and its invariants: [protocol reference](https://lipen.github.io/nexus-raw/reference/protocol/).
- The CLI that wraps this crate: [nexus-raw](https://crates.io/crates/nexus-raw).
- The user-facing site: <https://lipen.github.io/nexus-raw/>.
- License: MIT.
