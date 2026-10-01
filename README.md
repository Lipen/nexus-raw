<div align="center">

<img src="docs/assets/img/hero-terminal.png" alt="A recorded nxr session: publishing a version directory against the mock server" width="760">

# nexus-raw

`nxr` is curl for a Sonatype Nexus raw repository.

[![CI](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml/badge.svg)](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml)
[![Docs](https://github.com/Lipen/nexus-raw/actions/workflows/docs.yml/badge.svg)](https://github.com/Lipen/nexus-raw/actions/workflows/docs.yml)
[![crates.io](https://img.shields.io/crates/v/nexus-raw.svg)](https://crates.io/crates/nexus-raw)

[Docs](https://lipen.github.io/nexus-raw/) · [Install](#install) · [Quick start](#quick-start) · [Commands](#commands) · [README на русском](README.ru.md)

</div>

## Install

Requires Rust 1.85 or newer.

```bash
cargo install nexus-raw
```

From the repository instead (installs whatever is on the default branch, not a released version):

```bash
cargo install --git https://github.com/Lipen/nexus-raw nexus-raw --locked
```

From a checkout:

```bash
cargo install --path crates/nexus-raw --locked
```

The Node bindings are not on npm yet: they need their per-platform prebuilt packages first.

Check it landed with `nxr --version`.

## Quick start

Publish a version, name it with a channel, fetch it back and verify it offline:

```bash
BASE=https://nexus.example.com/repository/raw-main

# down refuses a version without manifest.json; it lists the names a consumer may fetch
printf '{"artifacts": ["app-1.4.0.zip"]}\n' > dist/1.4.0/manifest.json
nxr up dist/1.4.0/ "$BASE/1.4.0/"
nxr channel set "$BASE/latest" 1.4.0 --if-forward
V=$(nxr channel get "$BASE/latest")
nxr down "$BASE/$V/" vendor/prebuilt
nxr verify vendor/prebuilt
```

An interrupted transfer finishes by repeating the same command: complete parts are skipped.

## Commands

| Command | Does |
|:--------|:-----|
| `nxr get <URL> [-o FILE] [--continue]` | GET to a file (via `.part`, resume through Range) or stdout |
| `nxr put <URL> -f FILE [--sha]` | PUT bytes: `--sha` also PUTs the `.sha256` sibling |
| `nxr head <URL>` | status, size, content type |
| `nxr sha <FILE\|URL>` | streaming sha256 of a file or a remote object |
| `nxr up <SRC_DIR> <DST_URL> [--manifest F] [--no-sha] [--dry-run] [--claim-first NAME]` | scan → diff → PUT bytes + markers in parallel workers |
| `nxr down <SRC_URL> <DST_DIR> [--manifest F\|URL\|-] [--name N]... [--ls] [--fresh]` | enumerate → diff → stream+hash → rename + local marker |
| `nxr ls <URL> [--assets]` | version or object listing through the search API (experimental) |
| `nxr channel get <URL>` | print the current token (`unset` when empty) |
| `nxr channel set <URL> <TOKEN> [--if-forward]` | write a token; `--if-forward` accepts only forward moves in dotted-numeric order |
| `nxr verify <DIR> [--manifest F\|-]` | local bytes + marker + digest only, no network |
| `nxr doctor [URL]` | credentials, TLS, settings, reachability |

`down` enumerates explicitly: a `manifest.json` at the version URL, `--manifest`, repeatable `--name`, or best-effort `--ls`; with none of them it refuses.

Exit codes: 0 ok, 1 data problem, 2 misuse, 3 transport.
Every error prints a `hint:` line.
`--json` emits one JSON object per line on stdout.

## Credentials

Sources, in checked order: `-u user:pass`, then `NXR_AUTH` (base64 of `user:pass`), then `NXR_USERNAME` + `NXR_PASSWORD`.

```bash
export NXR_USERNAME="my-login"
export NXR_PASSWORD="my-password"

# or one value for CI: base64 of "my-login:my-password"
export NXR_AUTH="$(printf '%s:%s' 'my-login' 'my-password' | base64)"
```

`nxr doctor [URL]` names the source that resolved, without printing values.

## Docs

Guides and reference: <https://lipen.github.io/nexus-raw/>.
Sources live in [docs/](docs/); `just docs` serves the site locally.

## Contributing

Build, test, commit, protocol changes and the release checklist: [CONTRIBUTING.md](CONTRIBUTING.md).
Repository layout and the rules for changing things: [AGENTS.md](AGENTS.md).
`just check` runs the full gate; `just demo` runs the recorded session against the local mock server.

## License

MIT, see [LICENSE](LICENSE).
