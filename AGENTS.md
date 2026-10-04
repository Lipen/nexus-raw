# Working in nexus-raw

This repository ships nexus-raw, a general-purpose client for Sonatype Nexus raw storage: the `nxr` CLI, the `nexus-raw-core` Rust library, and the `mock-nexus` failure-scenario server that the conformance tests run against.
`nxr` is curl for a Nexus raw repository: URL in argv, credentials from `-u` or env, no config file, no profiles.
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
- `down` requires an enumeration source (`manifest.json` at the directory URL, `--manifest`, repeatable `--name`, or `--ls`) and refuses with `cannot enumerate` otherwise.
- The marker is always PUT after the bytes of the same name.
- The order between names is free.
- Names are relative paths of `[A-Za-z0-9._-]` segments.
  Only the `.sha256` suffix is reserved.
  `version.json`, `latest` and `nightly` are ordinary names with no protocol meaning.
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
| a mock scenario's exact behavior | `crates/mock-nexus/src/scenario.rs` doc comment on `Scenario` |
| exit codes and hints | `crates/nexus-raw-core/src/error.rs` |
| the lint/test gate | [Justfile](Justfile), [prek.toml](prek.toml) |
| the recorded session and the landing animation | `examples/demo/README.md` |
| the version agreement check | `scripts/version-check.sh` |

## Verify before reporting done

```bash
just check        # fmt + clippy + prek + tests: the full gate
just check-docs   # the docs site builds and every page resolves
just test         # unit and conformance suites alone
just nxr -- --help
just stand        # the real-Nexus battery (needs docker)
```

[CONTRIBUTING.md](CONTRIBUTING.md) carries the release checklist: one version for the workspace, `just version X.Y.Z`, a signed tag, and a workflow that publishes the crates in dependency order.

The conformance suites drive `nexus-raw-core` and the `nxr` binary against `mock-nexus` scenarios: the core suite through the facade, the CLI suite through the real binary.
The workspace also carries unit tests in the core library, unit tests in the napi bindings and doctests, and `cargo test --workspace` runs them all.
The scenario list in `mock-nexus` is the single source.
Do not fork it.
`--json` output shapes are fixed by golden tests in `crates/nexus-raw/tests/`.
New fields are additive.

## Layout

| Path | Role |
|:-----|:-----|
| `crates/nexus-raw-core/src/` | the protocol: `transport/` (client, retry), `primitive.rs` (get/put/head/sha), `sync/` (scan, diff, up, down, rm, mirror), `layout/` (channel, manifest, ls), `model/` (name, digest, sibling, state, pointer tokens), `config.rs` (per-invocation `Config`, no config file), `creds.rs`, `error.rs`, `events.rs`, and the `Nxr` facade as the single entry |
| `crates/nexus-raw/src/` | the `nxr` binary: `main.rs` (clap), `cmd/` (`primitives`, `transfer`, `layout`, `ls`, `verify`, `doctor`), `render/` (human, NDJSON) |
|`crates/mock-nexus/`|the mock server (std-only HTTP/1.1) with the failure scenarios, as a lib for Rust tests and a binary for humans and external test suites|
|`stand/real-nexus/`|the docker stand against a real Nexus: `just stand` locally, the same battery nightly in CI|
| `docs/`, `mkdocs.yml` | the documentation site (zensical, Material stack), served by `just docs` with live reload |
| `crates/nexus-raw-napi/` | the Node bindings (npm package `nexus-raw`): the CLI surface as promises over the `Nxr` facade, with `index.js`/`index.d.ts` entry files and napi CLI packaging |
| `examples/` | standalone external-consumer demos, excluded from the workspace: `examples/demo` (the mock-server stand behind the landing animation, `just demo`), `examples/node` (pnpm project on the npm package) and `examples/rust` (crate on the crates.io version); run with `just demo` / `just example-node` / `just example-rust` |

## Commits

Conventional, subject-only, no body: `scope: short imperative summary` with scopes `core`, `cli`, `mock`, `docs`, `chore`.
One feature or fix per commit, straight to master.
Never commit credentials or `target/`.
