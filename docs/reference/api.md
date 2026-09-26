# Rust API

`nexus-raw-core` is the protocol without a UI: the operations of the CLI as a small async API around one facade.
The crate is organized in layers, and the CLI is a thin shell over the top layer.

## Layers

| Layer | Modules | Content |
|:------|:--------|:--------|
| L0 transport + primitives | `transport`, `primitive` | retries, backoff, stall detection, TLS, auth, `get`/`put`/`head`/`sha` |
| L1 transfer | `sync` | directory up/down, the symmetric diff, sha-sibling markers, parallel workers, Range-resume |
| L2 layout helpers | `layout` | channels (token files with any name), manifests, search-based listings |
| L3 UX | the CLI crate | doctor, hints, human and NDJSON rendering |

Layers never import upward.
Everything below L3 is public API — you can embed just the transport, just the transfer engine, or the whole facade.

## Add the dependency

```toml
[dependencies]
nexus-raw-core = { path = "crates/nexus-raw-core" }   # inside this workspace
# or, once published:
# nexus-raw-core = "0.3"
```

## The facade

`Nxr` is the single entry point.
It is built from a `Config` — one invocation's settings, no config file:

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
            base: "https://nexus.example.com/repository/raw-main/1.4.0/".into(),
            tls_insecure: false,
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            auth: Some("Basic Y2ktYm90OnRva2Vu".into()), // resolved from -u or env by callers
        },
        tx,
    )?;

    // Self-sufficient GET of one object.
    nxr.get(
        "https://nexus.example.com/repository/raw-main/1.4.0/app.zip",
        Some("app.zip".into()),
        false,
    )
    .await?;

    // Verified upload of a directory: markers on by default.
    nxr.up("dist/1.4.0".as_ref(), None, true, None).await?;

    // Dropping the facade closes the event channel, so the renderer finishes.
    drop(nxr);
    renderer.await.unwrap();
    Ok(())
}
```

`Config::base` is the directory URL this invocation works on, normalized to a trailing `/`.
`auth` is the ready `Authorization` header value — build it from `-u user:pass`, `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD`, exactly as the CLI does.

## Operations

| Method | Returns | Mirrors |
|:--------|:--------|:--------|
| `get(url, out, cont)` | `GetOutcome` | `nxr get` |
| `put(url, src, sha)` | `(u64, Option<Digest>)` | `nxr put` |
| `head(url)` | `HeadInfo` | `nxr head` |
| `sha(src)` | `Digest` | `nxr sha` (`ShaSource::File` or `ShaSource::Url`) |
| `scan(dir)` | `Vec<ArtifactName>` | the local directory listing `up` starts from |
| `diff(dir, names, mode, markers)` | `Vec<Action>` | `up --dry-run` — the plan without transferring |
| `up(dir, names, gen_markers, plan)` | `Summary` | `nxr up` |
| `down(dir, enum_src, cont, plan)` | `Summary` | `nxr down` |
| `verify(dir, names)` | `Summary` | `nxr verify` — offline, emits only the summary event |
| `channel_get(url)` | `Option<String>` | `nxr channel get` — `None` on 404 |
| `channel_set(url, token, if_forward)` | `ChannelOutcome` | `nxr channel set` |
| `manifest_at_base()` | `Option<Manifest>` | the `manifest.json` convention behind `down` |
| `manifest_from(url)` | `Manifest` | `--manifest <url>` |
| `ls_versions()` | `Vec<String>` | `nxr ls` (experimental, search API) |
| `ls_assets()` | `Vec<ArtifactName>` | `nxr ls --assets` (experimental) |

`names: Option<Vec<ArtifactName>>` restricts a transfer to a manifest's names.
`None` scans the directory.
`plan: Option<Vec<Action>>` accepts a precomputed diff — print the plan, then execute exactly it.

## Enumeration

`down` refuses to guess what to fetch, so it takes an explicit source:

```rust
use nexus_raw_core::{Enumeration, Nxr};

async fn fetch_all(nxr: &Nxr) -> Result<(), nexus_raw_core::Error> {
    // The recommended path: manifest.json at the facade's base URL.
    let manifest = nxr.manifest_at_base().await?
        .ok_or_else(|| nexus_raw_core::Error::Misuse("no manifest.json at the base".into()))?;
    nxr.down("vendor/app".as_ref(), Enumeration::Manifest(manifest), true, None).await?;
    Ok(())
}
```

- `Enumeration::Manifest(Manifest)` — a parsed `{"artifacts": [...]}` list.
- `Enumeration::Names(Vec<ArtifactName>)` — explicit names, the `--name` path.
- `Enumeration::Search` — best-effort traversal through the server search API, the `--ls` path.

## Events

`Event` is the progress stream, one channel the facade emits into:

| Variant | Carries |
|:--------|:--------|
| `Plan` | `upload`, `download`, `skip` name lists |
| `ArtifactStarted` | name, direction, total size when known |
| `ArtifactBytes` | coalesced progress — at most one per 200 ms per name |
| `ArtifactDone` | name, `skipped` flag, final byte count |
| `Retrying` | name, attempt number, reason |
| `Summary` | `uploaded`, `downloaded`, `skipped`, `failed` |

`Event::to_json()` yields the exact NDJSON shapes the CLI prints, so an embedding UI and the CLI output cannot diverge.

## Errors

`nexus_raw_core::Error` is the whole taxonomy: `Mismatch`, `Incomplete`, `UnsafeName`, `Missing`, `Enumerate`, `Auth`, `Transport`, `Http`, `Misuse`.
`Error::exit_code()` maps it to the CLI's exit classes (0 ok, 1 data, 2 misuse, 3 transport) and `Error::hint()` returns the human hint the CLI renders on stderr.
`Verdict` (diff refusals) converts into `Error` with `From`, so a refused plan and a refused transfer look identical to a caller.

## Guarantees

- No protocol logic outside the crate: the CLI is flags and rendering only.
- Credentials never appear in `Event`s, error messages or `Display` impls.
- Markers, the write order, the classification and resume follow [the protocol](protocol.md) exactly.
