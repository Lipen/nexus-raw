# Working in nexus-raw

This repository ships nexus-raw, a general-purpose client for Sonatype Nexus raw storage: the `nxr` CLI, the `nexus-raw-core` Rust library, and the `mock-nexus` failure-scenario server that the conformance tests run against.
[README.md](README.md) is the user-facing entry.
The protocol summary lives in the `crates/nexus-raw-core/src/lib.rs` crate docs; the canonical protocol text is kept outside this repository.

## Know before you change anything

- The protocol is the contract.
  A divergence between code and the protocol text is a bug in the code.
  A protocol change is one PR: update the contract text, then every implementation (core, CLI, the future napi wrapper) in the same commit range.
- Completion is bytes plus the sha-sibling, digest matching.
  A divergent complete artifact is never overwritten.
  A claim is written once and never rewritten.
- The marker is always PUT after the bytes of the same name; the order between names is free.
- Credentials live only in env vars and memory: never argv, never logs, never `--json` output, never temp files in artifact directories, never the TOML config.
- TLS verification is on by default; `--tls-insecure` is the only way off.
- Exit codes have one home: `Error::exit_code` in `crates/nexus-raw-core/src/error.rs`.
  0 ok, 1 data, 2 misuse, 3 transport.
- The failure-scenario list is the conformance contract.
  It lives in `crates/mock-nexus/src/lib.rs` (`SCENARIOS`); a scenario added for one client implementation lands in the same PR as its test.
- Warnings are errors: `cargo clippy --workspace --all-targets -- -D warnings`.
- Markdown files and code comments: one sentence per line, no hard wrapping.
- This is a general-purpose utility: keep names, examples, defaults and docs neutral.

## Where to look

| Changing | Read |
|:---------|:-----|
| the wire protocol, layouts, errors, forbiddances | `crates/nexus-raw-core/src/lib.rs` crate docs |
| the core API (facade `Nxr`, events, actions) | `crates/nexus-raw-core/src/lib.rs` |
| CLI flags, config, credentials order, output examples | [README.md](README.md) |
| a mock scenario's exact behavior | `crates/mock-nexus/src/lib.rs` doc comment on `Scenario` |
| exit codes | `crates/nexus-raw-core/src/error.rs` |
| the lint/test gate | [Justfile](Justfile), [prek.toml](prek.toml) |

## Verify before reporting done

```bash
just check        # fmt + clippy + prek + tests: the full gate
just check-docs   # the docs site builds and every page resolves
just test         # unit and conformance suites alone
just nxr -- --help
```

The conformance tests drive `nexus-raw-core` and the `nxr` binary against `mock-nexus` scenarios.
The scenario list in `mock-nexus` is the single source; do not fork it.
`--json` output shapes are fixed by golden tests in `crates/nexus-raw/tests/`; new fields are additive.

## Layout

| Path | Role |
|:-----|:-----|
| `crates/nexus-raw-core/` | the protocol: names, digest, sibling, claim, state, diff, remote (reqwest), retry, up, down, pointer, creds, config, events, errors; the `Nxr` facade is the single entry |
| `crates/nexus-raw/` | the `nxr` binary: clap parsing, human and NDJSON rendering |
| `crates/mock-nexus/` | the mock server (std-only HTTP/1.1) with the nine failure scenarios; lib for Rust tests, binary for humans and external test suites |
| `docs/`, `mkdocs.yml` | the documentation site (zensical, Material stack); `just docs` serves it with live reload |
| `node/` | future home of the npm packaging; does not exist yet |

## Commits

Conventional, subject-only, no body: `scope: short imperative summary` with scopes `core`, `cli`, `mock`, `docs`, `chore`.
One feature or fix per commit, straight to master.
Never commit credentials or `target/`.
