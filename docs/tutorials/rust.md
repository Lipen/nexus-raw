# Rust quickstart

From an empty directory to a verified transfer in Rust, in about five minutes.
The library is `nexus-raw-core`: the operations of the `nxr` CLI without the UI, behind one facade named `Nxr`.
No Nexus server is needed: the repository's `mock-nexus` plays one.

The same operations from Node: [the Node quickstart](node.md).
From a shell instead of a library: [the terminal quickstart](ops.md).

## Prerequisites

- Rust 1.88+ through `rustup`.
- A running mock server, the next step.

## Start the mock

=== "From the repository checkout"

    ```bash
    cargo run -p mock-nexus -- atomic --port 8080
    ```

=== "From crates.io"

    ```bash
    cargo install mock-nexus
    mock-nexus atomic --port 8080
    ```

```console
listening http://127.0.0.1:8080
```

`atomic` is the correct-server scenario: PUT/GET/HEAD, `Range` resume, 404 on unknown paths.
Everything below talks to that process.

## Create the project

```bash
cargo new first-transfer-rs
cd first-transfer-rs
cargo add nexus-raw-core
cargo add tokio --features rt-multi-thread,macros,sync
```

## The program

Replace `src/main.rs` with:

```rust
use std::path::Path;
use std::time::Duration;

use nexus_raw_core::{ArtifactName, Config, Enumeration, Event, Nxr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A version directory: any files, the relative paths are the artifact names.
    std::fs::create_dir_all("dist/bom")?;
    std::fs::write("dist/app.bin", vec![0u8; 4096])?;
    std::fs::write("dist/bom/manifest.json", r#"{"artifacts":["app.bin"]}"#)?;
    std::fs::write(
        "dist/manifest.json",
        r#"{"artifacts":["app.bin","bom/manifest.json"]}"#,
    )?;

    // The facade: one config, one event channel, every operation.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let nxr = Nxr::new(
        Config {
            base: "http://127.0.0.1:8080/demo/1.0.0/".into(),
            tls_insecure: false,
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            auth: None,
        },
        tx,
    )?;
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            println!("{}", event.to_json());
        }
    });

    // Publish: up generates the .sha256 marker of every file, and down
    // checks each stream against the server marker before the file lands.
    let up = nxr.up(Path::new("dist"), None, true, None).await?;
    println!("up: uploaded {}, skipped {}", up.uploaded, up.skipped);

    // Fetch the version back by name.
    let down = nxr
        .down(
            Path::new("vendor"),
            Enumeration::Names(vec![
                ArtifactName::parse("app.bin")?,
                ArtifactName::parse("bom/manifest.json")?,
            ]),
            false,
        )
        .await?;
    println!(
        "down: downloaded {}, skipped {}",
        down.downloaded, down.skipped
    );

    drop(nxr);
    printer.await?;
    Ok(())
}
```

One config, one directory URL, two calls: `up` scans the directory, diffs it against the server, and uploads what moved, `down` fetches exactly the listed names.
The `Config` here is the defaults the CLI uses.
Credentials travel in `auth` as the ready `Authorization` header: `creds::resolve` builds it from `-u`-style pairs or `NXR_AUTH`, the same way the CLI does.

## Run it

```bash
cargo run
```

```console
{"download":[],"event":"plan","skip":[],"upload":["app.bin","bom/manifest.json","manifest.json"]}
{"done":4096,"event":"artifact","name":"app.bin","state":"uploading","total":4096}
…
{"downloaded":0,"event":"summary","failed":[],"removed":0,"skipped":0,"uploaded":3}
up: uploaded 3, skipped 0
{"download":["app.bin","bom/manifest.json"],"event":"plan","skip":[],"upload":[]}
…
down: downloaded 2, skipped 0
{"downloaded":2,"event":"summary","failed":[],"removed":0,"skipped":0,"uploaded":0}
```

The event lines interleave: the printer task drains the channel while the transfers run.
The `summary` event closes every transfer, and `failed` is the list to be empty.
Re-run the program: the plan empties, nothing transfers twice.

## Where to go next

- Every method of the facade, one table: [the Rust API](../reference/api.md).
- The same operations from Node: [the Node quickstart](node.md).
- The same operations from a shell: [the terminal quickstart](ops.md).
- What the wire actually carries: [the protocol](../reference/protocol.md).
