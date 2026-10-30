# Working in nexus-raw

This repository contains nexus-raw, a general-purpose client for Sonatype Nexus raw storage: the `nxr` CLI, the `nexus-raw-core` Rust library, the `nxr-tui` terminal browser, the `nxr-mount` read-only FUSE view (Linux), and the `mock-nexus` failure-scenario server that the conformance tests run against.
`nxr` is curl for a Nexus raw repository: URL in argv, credentials from `-u` or env, nothing required on disk.
A named remote (`-R/--remote`) reads `~/.config/nxr/config.toml`, and only an invocation that names one touches the file.
[README.md](README.md) is the user-facing entry.
The protocol summary lives in the `crates/nexus-raw-core/src/protocol.md` crate docs.
The canonical protocol text is kept outside this repository.

## Know before you change anything

- The protocol is the contract.
  A divergence between code and the protocol text is a bug in the code.
  A protocol change is one PR: update the contract text, then every implementation (core, CLI, the napi wrapper) in the same commit range.
- Completion is bytes plus the sha-sibling, digest matching.
  A divergent complete artifact is never overwritten.
  `up` writes markers by default and generates missing local siblings.
  `--no-sha` is the explicit opt-out.
- `down` takes its names from repeatable `--name`, `--names-from`, `--ls`, or a URL that ends at a file (the parent directory is the base, the leaf is the name). Without any source it refuses with `cannot enumerate`.
- The marker is always PUT after the bytes of the same name.
- The order between names is free.
- Names are relative paths of `[A-Za-z0-9._-]` segments.
  Only the `.sha256` and `.part` suffixes and a `.nxr`-prefixed segment (local staging and cache) are reserved.
  `version.json`, `latest` and `nightly` are ordinary names with no protocol meaning.
- The local scan refuses a symlink or any other non-regular entry: a link is never followed.
- A channel is any token file.
  Dotted-numeric order is the only comparison the tool imposes (`--if-forward`).
- Credentials come from `-u user:pass`, `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD`, in that order.
  `-u` is a deliberate argv exposure (doctor flags it).
  Values never appear in logs, `--json` output or error messages.
- TLS verification is on by default, and `--tls-insecure` is the only way off.
- Exit codes have one home: `Error::exit_code` in `crates/nexus-raw-core/src/error.rs`.
  0 ok, 1 data, 2 misuse, 3 transport.
  Every error carries a `hint:` line (`Error::hint`).
- The failure-scenario list is the conformance contract.
  It lives in `crates/mock-nexus/src/scenario.rs` (`SCENARIOS`).
  A scenario added for one client implementation lands in the same PR as its test.
- Warnings are errors: `cargo clippy --workspace --all-targets -- -D warnings`.
- Markdown files and code comments: one sentence per line, no hard wrapping.
- This is a general-purpose utility: keep names, examples, defaults and docs neutral.

## Where to look

| Changing | Read |
|:---------|:-----|
| the wire protocol, store shape, errors, forbiddances|`crates/nexus-raw-core/src/protocol.md` (the crate docs)|
| the core API (facade `Nxr`, `Enumeration`, events, actions) | `crates/nexus-raw-core/src/nxr.rs` |
| CLI flags, credentials order, output examples | [README.md](README.md) |
| the TUI (keys, config, presets, smoke) | `crates/nexus-raw-tui/README.md`, [docs/reference/tui.md](docs/reference/tui.md) |
| the mount (`nxr-mount`, FUSE semantics, its limits) | [docs/reference/mount.md](docs/reference/mount.md), `crates/nexus-raw-mount/src/` |
| a mock scenario's exact behavior | `crates/mock-nexus/src/scenario.rs` doc comment on `Scenario` |
| exit codes and hints | `crates/nexus-raw-core/src/error.rs` |
| the lint/test gate and the shared docs gate | [Justfile](Justfile), [prek.toml](prek.toml), `scripts/check-docs.sh` |
| the terminal output of a script | `scripts/ui.sh`, [CONTRIBUTING.md](CONTRIBUTING.md) |
| the recorded session and the landing animation | `examples/demo/README.md` |
| the version agreement check | `scripts/version-check.sh` |
| the tap formula and its drift check | `scripts/brew-formula.sh`, `scripts/check-brew-formula.sh` |
| refreshing an action pin | `.github/dependabot.yml` (weekly automatic path, except `dtolnay/rust-toolchain`: branch pins, refreshed by hand), `scripts/check-action-pins.sh` (the rule) |
| a quickstart's commands and outputs | `docs/tutorials/` (rust, node, ops) |
| the release pipeline, the tag rule and the published consumer gate | `.github/workflows/release.yml`, `scripts/check-release-tag.sh`, `scripts/verify-published.sh`, [CONTRIBUTING.md](CONTRIBUTING.md) |
| the benches and the baseline workflow | `bench/README.md`, the harness in `bench/src/lib.rs` |
| the Python reference client and its conformance suite | `clients/python/nxr.py`, `clients/python/test_conformance.py` (the skip table and the drift rule) |

## Verify before reporting done

```bash
just check        # wasm + prek (fmt and clippy included) + deny + tests: the full gate
just check-docs   # the docs site builds and every page resolves
just test         # unit and conformance suites alone
just nxr -- --help
just stand        # the real-Nexus battery (needs docker)
```

[CONTRIBUTING.md](CONTRIBUTING.md) carries the release checklist: `scripts/release.sh X.Y.Z --yes` bumps, pushes and dispatches (`just release X.Y.Z` alone is a preview, `--yes` runs the whole act), and the workflow publishes everything, then tags the tree it shipped.

The conformance suites drive `nexus-raw-core` and the `nxr` binary against `mock-nexus` scenarios: the core suite through the facade, the CLI suite through the real binary.
The workspace also carries unit tests in the core library, unit tests in the napi bindings and doctests, and `cargo test --workspace` runs them all.
The scenario list in `mock-nexus` is the single source.
Do not fork it.
`--json` output shapes are fixed by golden tests in `crates/nexus-raw/tests/`.
New fields are additive.

## Layout

| Path | Role |
|:-----|:-----|
| `crates/nexus-raw-core/src/` | the protocol: `transport/` (client, retry), `primitive.rs` (get/put/head/sha/sibling), `sync/` (scan, diff, up, down, rm, mirror), `layout/` (channel, ls), `model/` (name, digest, sibling, state, pointer tokens), `config.rs` (the per-invocation `Config`), `creds.rs`, `error.rs`, `events.rs`, and the `Nxr` facade as the single entry |
| `crates/nexus-raw/src/` | the `nxr` binary: `main.rs` (clap), `alias.rs` (the `-R` named-remote file), `cmd/` (`primitives`, `transfer`, `layout`, `doctor`), `render/` (human, NDJSON) |
|`crates/mock-nexus/`|the mock server (std-only HTTP/1.1) with the failure scenarios, as a lib for Rust tests and a binary for humans and external test suites|
|`stand/real-nexus/`|the docker stand against a real Nexus: `just stand` locally, the same battery nightly in CI|
| `docs/`, `mkdocs.yml` | the documentation site (zensical, Material stack), served by `just docs` with live reload |
| `crates/nexus-raw-napi/` | the Node bindings (npm package `@nexus-raw/nxr`, plus the unscoped `nexus-raw` bridge): the transfer, layout and service-metadata surface as promises over the `Nxr` facade (the operator commands `audit`, `mv`, `doctor` and the `service status/repo/eula` probes stay CLI-only), with `index.js`/`index.d.ts` entry files and napi CLI packaging |
| `crates/nexus-raw-tui/` | the `nxr-tui` binary: a terminal browser over raw repositories (tabs, presets, filters, downloads), a lib with the state machine, rendering and config plus a thin bin |
| `crates/nexus-raw-mount/` | the `nxr-mount` binary: a read-only FUSE view of a repository (Linux, pure-Rust `fuser`, no libfuse at build time), a companion to `nxr` and a second consumer of the core facade |
| `examples/` | standalone external-consumer demos, excluded from the workspace: `examples/demo` (the mock-server stand behind the landing animation, `just demo`), `examples/node` (pnpm project on the npm package), `examples/panel` (the web panel on the napi bindings, `just panel`), `examples/wasm-sandbox` (the browser sandbox, `just wasm`) and `examples/rust` (crate on the crates.io version). Run them with `just demo` / `just example-node` / `just panel` / `just wasm` / `just example-rust` |
| `bench/` | the opt-in benchmark ecosystem, a standalone workspace excluded from the gate: criterion benches over the core (`scan`, `diff`, `up`, `down`, `down-slow`) and the real binary (`cli`), the environment-driven dataset and worker ladders, the deterministic tree generator, and the report pipeline (`just bench-report`, `just bench-compare`). The workflow and the measurement rules live in `bench/README.md` |
|`clients/python/`|the stdlib-only reference client and its conformance suite against the mock scenario table: every scenario is tested or explicitly skip-listed, an unclassified scenario fails the suite. ruff and ty gate it, and `just check` runs the suite|

## Commits

Conventional, subject-only, no body: `scope: short imperative summary` with scopes `core`, `cli`, `mock`, `tui`, `docs`, `chore`.
One feature or fix per commit, straight to master.
Never commit credentials or `target/`.
