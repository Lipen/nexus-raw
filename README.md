# nexus-raw

<p align="center"><img src="docs/assets/img/hero-terminal.png" alt="A real nxr session: publish, name with a channel, consume, verify" width="720"></p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85%2B-dea584?style=flat-square" alt="rust 1.85+">
  <img src="https://img.shields.io/badge/protocol-claim__version%201-3f7e6e?style=flat-square" alt="protocol claim_version 1">
  <img src="https://img.shields.io/badge/config%20files-none-2ea44f?style=flat-square" alt="no config files">
  <img src="https://img.shields.io/badge/exits-0%20%7C%201%20%7C%202%20%7C%203-blue?style=flat-square" alt="exit codes 0/1/2/3">
</p>

`nxr` is curl for a Sonatype Nexus raw repository.
Primitives with retries, stall detection and TLS on.
Verified directory transfers on top.
Channel refs and manifests above those.
Every call is self-sufficient: URL in argv, credentials from `-u` or the environment.
No config file, no profiles.
The `nexus-raw-core` crate exposes the same operations as a Rust library.
Zero server-side components.

Features:

- `get`, `put`, `head`, `sha` — curl-grade primitives, digest computed on the fly.
- `up`, `down` — directory transfers with the symmetric diff, parallel workers and Range-resume (`.part` files, 206).
- sha-sibling markers in `sha256sum -c` format: `up` writes and generates them by default, `--no-sha` opts out.
- `down` enumerates explicitly: `manifest.json` at the version URL, `--manifest`, repeatable `--name`, or best-effort `--ls`.
- `channel get|set` — token files at any name, with a dotted-numeric `--if-forward` guard.
- `verify` — offline check of bytes, markers and digests.
- `doctor` — credentials, TLS, reachability.
- TLS verification on by default, `--tls-insecure` is the only off-switch.
- `--json`: one JSON object for simple commands, NDJSON events for transfers.
- Exit codes 0/1/2/3, and every error prints a `hint:` line on stderr.

## Install

```bash
cargo install --path crates/nexus-raw    # the nxr binary
```

## Credentials

`-u user:pass` wins, then the environment: `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD` (set together or not at all).
That is the whole list — the alias and the default URL live in your shell or CI, not in a config file.

```bash
printf 'ci-bot:%s' "$TOKEN" | base64
export NXR_AUTH="Y2ktYm90OnRva2Vu"
```

`-u` is visible in `ps`.
The env paths are the CI choice.
`nxr doctor` reports which source resolved, without printing values.

## Quickstart

```bash
BASE=https://nexus.example.com/repository/raw-main

# publish a version directory: markers are generated, verified and uploaded by default
nxr up dist/1.4.0/ "$BASE/1.4.0/"

# name it — a channel is a token file at any name
nxr channel set "$BASE/latest" 1.4.0 --if-forward

# fetch it elsewhere; manifest.json in the version directory drives the enumeration
nxr down "$BASE/1.4.0/" vendor/prebuilt --continue

# no manifest? name what you need
nxr down "$BASE/1.4.0/" vendor/prebuilt --name app.zip

# check a local directory offline; plan before transferring
nxr verify vendor/prebuilt
nxr up --dry-run dist/1.5.0/ "$BASE/1.5.0/"
```

An interrupted transfer is finished by repeating the same command: `up` skips what is already complete, `down --continue` resumes from part files through `Range: bytes=N-`.

## Commands

| Command | Does |
|:--------|:-----|
| `nxr get <URL> [-o FILE] [--continue]` | GET to a file (via `.part`, resume through Range) or stdout |
| `nxr put <URL> -f FILE [--sha]` | PUT bytes — `--sha` also PUTs the `.sha256` sibling |
| `nxr head <URL>` | status, size, content type |
| `nxr sha <FILE\|URL>` | streaming sha256 of a file or a remote object |
| `nxr up <SRC_DIR> <DST_URL> [--manifest F] [--no-sha] [--dry-run]` | scan → diff → PUT bytes + markers in parallel workers |
| `nxr down <SRC_URL> <DST_DIR> [--manifest F\|URL\|-] [--name N]... [--ls] [--continue]` | enumerate → diff → stream+hash → rename + local marker |
| `nxr ls <URL> [--assets]` | version or object listing through the search API (experimental) |
| `nxr channel get <URL>` | print a channel token (`unset` when empty) |
| `nxr channel set <URL> <TOKEN> [--if-forward]` | write a token, forward-only in dotted-numeric order on guard |
| `nxr verify <DIR> [--manifest F\|-]` | local bytes + marker + digest only, no network |
| `nxr doctor [URL]` | credentials, TLS, settings, reachability |

Exit codes: 0 ok, 1 data (`mismatch`, `incomplete`, `missing`, `cannot enumerate`), 2 misuse, 3 transport (network, auth, TLS, 5xx).
A divergent complete artifact is refused, never overwritten.

## Layout

| Path | For |
|:-----|:----|
| `crates/nexus-raw-core/` | the Rust library, in four layers: `transport` + `primitive`, `sync`, `layout`, and the `Nxr` facade |
| `crates/nexus-raw/` | the `nxr` binary: flags, rendering and exit codes only, no protocol logic |
| `crates/mock-nexus/` | the mock server with the failure-scenario table, the conformance fixture |
| `docs/` + `mkdocs.yml` | the documentation site (zensical), served by `just docs` |
| `node/` | future home of the npm packaging, which does not exist yet |

## Development

```bash
just check        # fmt + clippy + prek + tests
just test         # unit and conformance suites
just mock atomic --port 8080
just nxr -- up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/
```

User documentation lives in [docs/](docs/index.md) and renders as a site:

```bash
just docs     # http://localhost:8000, live reload
```

The Russian readme: [README.ru.md](README.ru.md).
