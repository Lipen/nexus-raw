# Integrate nexus-raw

Three depths, one contract: URL in argv, credentials from the environment or the call, verified bytes, refusals instead of overwrites.
Start with the CLI and descend only when you need the API in-process.

## As the CLI

```bash
cargo install nexus-raw
```

From a checkout instead, if you want the revision your code is building against: `cargo install --path crates/nexus-raw`.

Credentials resolve from `-u user:pass`, then `NXR_AUTH` (base64 of `user:pass`), then `NXR_USERNAME` + `NXR_PASSWORD`, in that order.
The sources, the precedence and the argv caveat are in the [CLI reference](../reference/cli.md#credentials-and-urls).

The command surface, one line each, all details in the [CLI reference](../reference/cli.md):

| Commands | Do |
|:---------|:---|
| `up`, `down` | verified directory transfers over the symmetric diff, resume included |
| `mirror` | pour a version from one repository into another: read the source, write the destination through the `up` rules |
| `get`, `put`, `head`, `sha` | curl-grade primitives, digest computed on the fly |
| `channel get`, `channel set` | name versions through token files, with the forward-only guard |
| `ls` | the raw-tree listing of a directory URL, any depth (`--assets`: flat artifact names), search-API based, best-effort |
| `verify` | offline check of bytes, markers and digests |
| `diff` | the local-against-storage delta report, nothing written |
| `rm` | delete an enumerated version, marker first, 404 is success |
| `mv` | `mirror` plus the delete: nothing is removed until the pour converges |
| `point --clear` | delete one pointer file, idempotently |
| `service repos` | list the server's repositories with format and writability |
| `doctor` | credentials, TLS and reachability, without printing secrets |

Scripts gate on the exit codes, not on output text: `0` continues, `1` is a data problem, `2` is a broken invocation, `3` is transport trouble.
The cause-and-fix table is on [when it breaks](troubleshoot.md#exit-codes-cause-and-fix).
The classes and their hints are pinned in [errors and exit codes](../reference/errors.md).

For scripting, `--json` turns every command into machine output: one JSON object for simple commands, one NDJSON event per line for transfers.
The event shapes are fixed, and the [CI page](ci.md#ndjson-events) shows a `jq` gate built on the `summary` event.

Shell completion comes from the binary itself: `nxr complete <shell>` prints the script, one line enables it per shell.

```bash
source <(nxr complete bash)                              # ~/.bashrc
nxr complete zsh > "${fpath[1]}/_nxr"                    # then restart the shell
nxr complete fish > ~/.config/fish/completions/nxr.fish
nxr complete powershell | Out-String | Invoke-Expression # or the line in the profile
```

The generated script names the commands and flags of the build that printed it, so refresh a file-based install after a binary upgrade.

## As a Rust library

`nexus-raw-core` is the same protocol without a UI.
It is on crates.io.
Take it as a version dependency:

```toml
[dependencies]
nexus-raw-core = "0.8"
```

A git pin or a vendored copy still works for special cases.
Vendoring has its own section below.

The [`Nxr`](../reference/api.md#the-facade) facade is the single entry point: build it from a `Config` and an event channel, then call methods that mirror the CLI commands one-to-one.
Progress arrives as [`Event`](../reference/api.md#events) objects over a `tokio::sync::mpsc::UnboundedReceiver`: drain it while the operation runs, or drain the buffered queue afterwards.

```rust
use std::time::Duration;
use nexus_raw_core::{Config, Enumeration, Nxr};

let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

// Render progress while the transfer runs; the channel closes when the
// facade drops, which ends this loop.
let renderer = tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        println!("{}", event.to_json());
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
        auth: Some("Basic Y2ktYm90OnRva2Vu".into()),
    },
    tx,
)?;

// The enumeration source is mandatory, exactly like `nxr down`.
let summary = nxr
    .down("vendor/app/".as_ref(), Enumeration::Search, false)
    .await?;
println!("downloaded {}, skipped {}", summary.downloaded, summary.skipped);

drop(nxr);
renderer.await?;
```

The snippet is kept in sync by hand.
The runnable version lives in [examples/rust](https://github.com/Lipen/nexus-raw/tree/master/examples/rust), a standalone crate outside this workspace: `just example-rust` builds the mock server and runs the whole publish-and-consume loop against it, on the crates.io build of the core.
The guided tour of the API, layers included: [the Rust API](../reference/api.md).

## As Node bindings

The `nexus-raw-napi` crate ships a subset of the surface to Node as promises: the npm package name is `nexus-raw`, built on napi-rs 3.
Install from the registry:

```bash
npm install @nexus-raw/nxr
```

Prebuilt addons ship for `linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64` and `win32-x64-msvc`, all under the `@nexus-raw` scope.
On Alpine (musl) there is no native addon: use the static CLI from the releases page, or build the addon from a repository checkout (below).
The CLI also ships a Windows binary: `nxr-windows-x64.tar.gz` in the [GitHub release](https://github.com/Lipen/nexus-raw/releases/latest).
A runnable consumer lives in [examples/node](https://github.com/Lipen/nexus-raw/tree/master/examples/node): it installs the package from the registry and drives the whole publish-and-consume loop against the mock server:

```bash
cd examples/node && pnpm install
node publish-and-consume.mjs
```

To consume a locally built addon instead of the registry build, build the addon, flip the dependency to `link:../../crates/nexus-raw-napi` and run `pnpm install` again.

`npm run build:debug` runs the napi CLI over a cargo build of the crate and leaves the platform addon (`*.node`), the generated loader (`binding.cjs`) and the generated declarations (`binding.d.ts`) next to the crate.

Every command is one self-sufficient call, and an optional `onEvent` callback receives the core's NDJSON events as parsed JSON objects.
The ordering contract matters for tests: the promise settles only after every event has been handed to the callback, including on failure.
The events that led to the error arrive first, then the promise rejects with an `Error` carrying `exitCode` and `hint`.
Events still cross a thread boundary, so the last callback invocation may run an instant after the promise resolved.
Tests that assert on collected events drain the JS queue first, as `crates/nexus-raw-napi/smoke.mjs` does.

```js
import { up } from 'nexus-raw'

const summary = await up('dist/1.4.0', 'https://nexus.example.com/repository/raw-main/1.4.0/', {
  claimFirst: 'version.json',
  onEvent: (event) => console.log(event),
})
```

The typed surface is `crates/nexus-raw-napi/index.d.ts`, maintained by hand.
`binding.d.ts` is the mechanically generated twin, and `check-dts.mjs` next to them fails when the two drift.

### Vendoring

When a copy must live in your tree, vendor the whole workspace (or at least `crates/nexus-raw-core` and `crates/nexus-raw-napi`) and build the addon for your own platform targets.
Two artifacts of this repository are the contract a vendored copy inherits:

- the crate docs of `nexus-raw-core` (`crates/nexus-raw-core/src/protocol.md`) are the behavioral summary of the protocol;
- the [conformance suites](../explanation/conformance.md) are its contract tests, and the scenario list in `crates/mock-nexus` is the single table they run against.

A vendored copy is expected to keep those suites green: `cargo test --workspace` runs them, and a scenario added upstream lands in the same change as its test.

## Next steps

- The two flows the tools exist for: [publish a version](publish.md) and [consume artifacts](consume.md).
- Flags, transcripts and output shapes: [the CLI reference](../reference/cli.md).
- Embedding deeper in Rust: [the API](../reference/api.md).
- What every level above is pinned against: [conformance](../explanation/conformance.md).
