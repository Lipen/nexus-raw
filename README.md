# nexus-raw

A general-purpose client for a Sonatype Nexus raw repository.
`nxr` uploads and downloads version directories described by a `claim.json`, verifies sha256 sibling markers, and resumes interrupted transfers by diffing local and remote state.
The `nexus-raw-core` crate exposes the same operations as a Rust library.
Zero server-side components.

Features:

- `up`, `down`, local `verify`, transfer `diff`, `ls`, pointer updates (`point`).
- Completion is bytes plus a `<name>.sha256` marker in `sha256sum -c` format.
- Parallel transfers (8 workers by default), retries with backoff, stall detection.
- TLS verification on by default; credentials only from env vars.
- `--json` NDJSON output; stable exit codes 0/1/2/3.

## Install

```bash
cargo install --path crates/nexus-raw    # the nxr binary
```

## Setup

The config holds only URLs, at `~/.config/nxr/config.toml` (override with `$NXR_CONFIG`):

```toml
default_profile = "release"

[release]
url = "https://nexus.example.com/repository/raw-main/"

[dev]
url = "http://localhost:8080/repository/raw-dev/"
tls_insecure = true
```

Credentials resolve from env, in order:

1. `NXR_<PROFILE>_AUTH` — base64 `user:pass`, uppercased profile name, `-` becomes `_`.
2. `NXR_AUTH` — the same, for every profile.
3. `NXR_USERNAME` + `NXR_PASSWORD`.

Passwords in the TOML config are refused.
Use `--base <url>` instead of `--profile` to skip the config entirely.

## Quickstart

```bash
# publish a version directory that contains claim.json + artifacts + .sha256 markers
nxr up --profile release --dir dist/1.4.0
nxr point latest 1.4.0 --if-newer --profile release

# fetch a version through a pointer
nxr down --profile release --pointer latest --dir vendor/prebuilt

# check a local build without touching the network
nxr verify --dir dist/1.4.0

# plan against the server, nothing written
nxr diff --profile release --dir dist/1.4.0
nxr up --profile release --dir dist/1.4.0 --dry-run
```

An interrupted transfer is resumed by repeating the same command.

## Commands

| Command | Does |
|:--------|:-----|
| `nxr up --dir <dir> [--names <file>] [--dry-run]` | claim (drift check) → diff → PUT bytes + markers |
| `nxr down --dir <dir> (--version <v> \| --pointer <latest\|nightly>) [--only <name>]...` | GET claim → diff → tmp+hash → rename → marker; resume is always on |
| `nxr verify --dir <dir>` | local bytes + marker + digest only, no network |
| `nxr diff --dir <dir>` | the plan against the server, symmetric for up and down |
| `nxr ls [--version <v>]` | per-name remote states, or the version list (REST search, experimental) |
| `nxr point <latest\|nightly> <version> [--if-newer]` | atomic pointer PUT; `--if-newer` is forward-only |

Exit codes: 0 ok, 1 data (mismatch, incomplete, claim drift, missing), 2 misuse, 3 transport (network, auth, TLS, 5xx).
`--json` emits NDJSON events of the same structs the core uses; pipe it to `jq`.

## Layout

| Path | For |
|:-----|:----|
| `crates/nexus-raw-core/` | the Rust library: the `Nxr` facade, typed errors, event stream |
| `crates/nexus-raw/` | the `nxr` binary: flags and rendering only, no protocol logic |
| `mock/mock-nexus/` | the mock server with the failure-scenario table, the conformance fixture |
| `node/` | future home of the npm packaging; does not exist yet |

## Development

```bash
just check        # fmt + clippy + prek + tests
just test         # unit and conformance suites
just mock atomic --port 8080
just nxr -- up --base http://127.0.0.1:8080/ --dir dist/1.4.0
```

User documentation lives here; reference documentation is planned under `docs/` (mdbook).
The Russian readme: [README.ru.md](README.ru.md).
