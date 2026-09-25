# nexus-raw

Reliable artifact delivery over a bad channel: claim, sha-sibling markers, symmetric diff, resume.
One static binary `nxr` for everyone and the Rust library `nexus-raw-core`; zero server-side components.
It speaks the claim_version 1 raw layout described in [SPEC.md](SPEC.md) (harbor, panda-sdk §6) and replaces a dozen ad-hoc curl scripts: one auth policy, one retry policy, TLS verified by default, credentials never in argv.

## Install

```bash
cargo install --path crates/nexus-raw    # the nxr binary
```

## Setup

Config holds only URLs, at `~/.config/nxr/config.toml` (override with `$NXR_CONFIG`):

```toml
default_profile = "panda"

[panda]
url = "https://nexus.example/repository/koala-raw/panda/"

[dev]
url = "http://localhost:8080/raw/dev/"
tls_insecure = true
```

Credentials resolve from env, in order: `NXR_<PROFILE>_AUTH` (base64 `user:pass`), `NXR_AUTH`, `NXR_USERNAME` + `NXR_PASSWORD`, `OPENLAB_USERNAME` + `OPENLAB_PASSWORD`.
Passwords in the TOML config are refused.

## Quickstart

```bash
# producer
nxr up --profile panda --dir dist/1.14.0          # an interruption is fine: repeat the same command
nxr point latest 1.14.0 --if-newer --profile panda

# consumer
nxr down --profile panda --pointer latest --dir third-party/panda/prebuilt

# check a local build without touching the network
nxr verify --dir dist/1.14.0

# plan against the server, nothing written
nxr diff --profile panda --dir dist/1.14.0
nxr up --profile panda --dir dist/1.14.0 --dry-run
```

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
| [SPEC.md](SPEC.md) | the canonical protocol: layouts, markers, diff table, errors |
| `crates/nexus-raw-core/` | the Rust library: the `Nxr` facade, typed errors, event stream |
| `crates/nexus-raw/` | the `nxr` binary |
| `mock/mock-nexus/` | the mock server with the failure-scenario table (`just mock atomic`) |

## Development

```bash
just check        # fmt + clippy + prek + tests
just test
just mock atomic --port 8080
just nxr -- up --base http://127.0.0.1:8080/ --dir dist/1.14.0
```

The full story: [AGENTS.md](AGENTS.md).
The Russian readme: [README.ru.md](README.ru.md).
