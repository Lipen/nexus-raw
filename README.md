<div align="center">

<a href="https://lipen.github.io/nexus-raw/how-to/demo/">
<img src="docs/assets/img/session.svg" alt="A recorded nxr session: publishing a version directory, naming it with a channel, fetching it back and verifying it against the mock server" width="760">
</a>

# nexus-raw

**Publish and fetch version directories in a Sonatype Nexus raw repository.
Transfers run in parallel, every artifact carries a sha256 marker, and an interrupted transfer resumes.**

`nxr` is a single static binary and needs nothing installed on the server.
Each call takes its URL as an argument and reads credentials from the call or from the environment.

[![CI](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml/badge.svg)](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/nexus-raw.svg)](https://crates.io/crates/nexus-raw)
[![npm](https://img.shields.io/npm/v/@nexus-raw/nxr.svg)](https://www.npmjs.com/package/@nexus-raw/nxr)

[Docs](https://lipen.github.io/nexus-raw/) · [Install](#install) · [Quick start](#quick-start) · [Common commands](#common-commands) · [CI](#ci) · [README на русском](README.ru.md)

</div>

## Overview

A raw repository stores objects over plain HTTP: one request per file.
A release is a directory of files that must all arrive, must stay verifiable after the fact, and must survive a pipeline that stops halfway.

Most pipelines do this with a shell script around `curl`.
Such a script accumulates retries, stall detection, digest checks and temporary files, and every project that publishes artifacts ends up maintaining its own copy.

`nxr` defines the same behavior once, with fixed semantics:

| Requirement | Script around `curl` | `nxr` |
|:---|:---|:---|
| parallel transfer | `xargs -P` | worker pool, `--workers N` |
| skip files already on the server | a custom listing comparison | symmetric diff on every run |
| a digest per artifact | a second request piped to `sha256sum -c` | `<name>.sha256` markers checked during the transfer |
| resume after an interruption | `curl -C -` per URL, without a digest check | hidden part files, `Range` resume, digest re-checked |
| protection against overwriting | none | a complete object with a different digest is rejected |
| separation of failure classes | one exit code | 1 data, 2 misuse, 3 transport, plus a `hint:` line |

Publishing a directory is then one command: `nxr up dist/1.4.0/ "$BASE/1.4.0/"`.

## Install

The binary is published as prebuilt archives, as a Homebrew formula, as an npm package that includes the `nxr` command, and as a container image.
Any of the following installs the same binary, and `nxr --version` prints the installed version.

```bash
# Homebrew, macOS and Linux
brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
brew install lipen/nexus-raw/nexus-raw

# a prebuilt static archive, without a compile step
cargo binstall nexus-raw

# the npm package, which also puts `nxr` on PATH
npm install @nexus-raw/nxr

# the container image
docker run --rm ghcr.io/lipen/nexus-raw:latest --version
```

The archives for Linux x64 and arm64, macOS Intel and Apple silicon, and Windows x64 are attached to [GitHub Releases](https://github.com/Lipen/nexus-raw/releases/latest), together with a `SHA256SUMS` file.
Installation from source, the archive list and the container details are below.

<details>
<summary>Install from source, the archive list, and the container</summary>

The crate builds from source and requires Rust 1.88 or newer:

```bash
cargo install nexus-raw
```

From the repository, or from an existing checkout:

```bash
cargo install --git https://github.com/Lipen/nexus-raw nexus-raw --locked
cargo install --path crates/nexus-raw --locked
```

The release archives, one file per platform:

| File | Platform |
|:-----|:---------|
| `nxr-linux-x64.tar.gz` | Linux x86_64, statically linked |
| `nxr-linux-aarch64.tar.gz` | Linux arm64, statically linked |
| `nxr-darwin-x64.tar.gz` | macOS Intel |
| `nxr-darwin-arm64.tar.gz` | macOS Apple silicon |
| `nxr-windows-x64.tar.gz` | Windows x86_64 |

A workflow that needs a fixed release can install one through `taiki-e/install-action`, replacing `<version>` with a release number:

```yaml
- uses: taiki-e/install-action@v2
  with:
    tool: nexus-raw@<version>
```

The container runs the same static binary on a distroless base.
Mount the working directory to pass local paths:

```bash
docker run --rm -v "$PWD:/w" -w /w ghcr.io/lipen/nexus-raw:latest up ./dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
```

The image variants and the cache layout are described in [run it as a container](docs/how-to/containers.md).

</details>

## Quick start

Five commands cover a release cycle: publish the directory, name the version, read the name back, fetch the files, and verify them.
`BASE` is the repository URL.

```bash
BASE=https://nexus.example.com/repository/raw-main

# every file under dist/1.4.0/ is uploaded in parallel, each artifact
# followed by a generated .sha256 marker
nxr up dist/1.4.0/ "$BASE/1.4.0/"

# a channel is a token file
# --if-forward allows only forward moves
nxr channel set "$BASE/latest" 1.4.0 --if-forward

# consumers read the current version from the channel
V=$(nxr channel get "$BASE/latest")

# the listed names are downloaded, hashed during the transfer and checked
# against the server marker before each file gets its final name
printf 'app-1.4.0.zip\nmanifest.json\n' > names.txt
nxr down "$BASE/$V/" vendor/prebuilt --names-from names.txt

# local bytes, markers and digests are checked without network access
nxr verify vendor/prebuilt
```

The output is abbreviated here:

```console
$ nxr up dist/1.4.0/ "$BASE/1.4.0/"
plan: 4 to upload, 0 to download, 0 up to date
↑ manifest.json ok
↑ pinned.xml ok
↑ bom/linux-x86_64.json ok
↑ app-1.4.0.zip ok
uploaded 4, downloaded 0, skipped 0

$ nxr down "$BASE/1.4.0/" vendor/prebuilt --names-from names.txt
plan: 0 to upload, 2 to download, 0 up to date
↓ app-1.4.0.zip ok
↓ manifest.json ok
uploaded 0, downloaded 2, skipped 0

$ nxr verify vendor/prebuilt
verify: 2 ok, FAILED: none
```

A repeated command transfers only the names that are still missing.
The [get-started page](docs/get-started.md) runs the same session against a local mock server, including an interrupted transfer.

## How it works

The protocol has five concepts.
An invocation needs the URL, the local files and the markers stored next to the artifacts.

- **Version directory.** Every relative path under the source directory becomes an artifact name, so `dist/1.4.0/bom/x.json` is stored as `bom/x.json` under the destination URL.
- **Marker.** An artifact is complete when its bytes and a `<name>.sha256` sibling in `sha256sum -c` format are both present.
  `up` generates missing markers and uploads them after the artifacts, `down` checks each stream against the server marker, and `verify` checks the pair offline.
- **Symmetric diff.** Both directions compare the local tree with the remote listing before transferring and move only the difference, so a repeated command completes an interrupted run instead of repeating it.
- **Channel.** A channel is a token file at any URL, such as `latest` or `stable`.
  `--if-forward` compares tokens in dotted-numeric order and rejects a move to an older token.
- **Part file.** An interrupted download remains in a hidden `.nxr-part-<hash>` file and never reaches its final path in a partial state.
  A repeated command continues it with one `Range` request.

A complete artifact whose local digest differs from the server marker is reported and not overwritten.

## Common commands

| Command | Description |
|:--------|:------------|
| `nxr up <DIR> <URL>` | upload a directory: scan, diff, then PUT the bytes and markers in parallel |
| `nxr down <URL> <DIR>` | download the listed artifacts, verified, with resume |
| `nxr verify <DIR>` | check local bytes, markers and digests without network access |
| `nxr diff <DIR> <URL>` | report differences (`same`, `missing-local`, `missing-remote`, `diverged`) without writing anything. Exit code 1 when a difference exists. |
| `nxr ls <URL>` | list a directory URL at any depth. `--assets` prints flat artifact names |
| `nxr channel get\|set` | read a channel token, or write one with `--if-forward` |
| `nxr doctor [URL]` | report credentials, TLS settings and reachability without printing secrets |
| `nxr get\|put\|head\|sha` | the curl primitives, with retries, stall detection and digests |

`down`, `diff` and `rm` require a name source: `--name`, `--names-from FILE`, or `--ls` when the server has the search API.
A URL that ends at a file is the one exception: `nxr down "$BASE/1.4.0/app.zip" vendor/` downloads that file.

<details>
<summary>Remaining commands and global flags</summary>

| Command | Description |
|:--------|:------------|
| `nxr clone <URL> <DIR>` | clone a whole repository, with an optional fact cache that skips re-hashing unchanged files |
| `nxr audit <URL>` | report integrity problems: orphans, dangling and unparseable markers, disagreeing facts. `--bytes` verifies every object, `--sample N` verifies a deterministic sample |
| `nxr mirror <SRC> <DST>` | copy a version between two repositories, using the same diff and rejection rules |
| `nxr mv <SRC> <DST>` | mirror a version and then delete it at the source, after the copy converged |
| `nxr rm <URL>` | delete a version: a plan by default, `--yes` deletes markers first and bytes after them, a 404 counts as success |
| `nxr point <URL>` | delete one pointer file. Absence is not an error. |
| `nxr service repos\|status\|repo\|assets\|eula <URL>` | the server REST API: repository list, liveness, settings, search snapshot, EULA state |
| `nxr complete <shell>` | shell completion for bash, zsh, fish or PowerShell |

The global flags are `-u user:pass`, `-R ALIAS`, `--workers N`, `--retry N`, `--connect-timeout-secs`, `--stall-secs`, `--tls-insecure`, `--ca-bundle FILE`, `--json`, `-q` and `-v`.

The flags, output formats and edge cases are documented in the [CLI reference](docs/reference/cli.md).
`nxr <command> --help` describes the installed binary.

</details>

## CI

A GitHub Actions workflow can use the composite action from this repository:

```yaml
- uses: Lipen/nexus-raw@v0.24.0
  with:
    command: up
    args: ./dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
    auth: ${{ secrets.NEXUS }}     # user:password, exported through the environment
```

Exit codes separate failure classes: `0` success, `1` a data problem that a retry will reproduce, `2` an invalid invocation, `3` a transport problem after which a later run is expected to succeed.
Because transfers resume, repeating the command is safe:

```bash
nxr up "dist/$VERSION/" "$BASE/$VERSION/" || status=$?
case ${status:-0} in
  0) echo published ;;
  3) sleep 60; nxr up "dist/$VERSION/" "$BASE/$VERSION/" ;;
  *) exit 1 ;;
esac
```

With `--json` a transfer prints one NDJSON event per line and ends with a `summary` event that a job can gate on.
Credentials come from masked CI variables, either `NXR_USERNAME` and `NXR_PASSWORD` or a single `NXR_AUTH` holding the base64 of `user:password`.

<details>
<summary>Other CI topics</summary>

The same archives can be installed through the upstream action, which applies its own checksum and attestation checks:

```yaml
- uses: taiki-e/install-action@v2
  with:
    tool: nexus-raw
```

GitLab CI, a `jq` gate on the `summary` event, masked variables and a retry loop are described in [use in CI](docs/how-to/ci.md).
The exit-code taxonomy with causes and fixes is in [errors and exit codes](docs/reference/errors.md).

</details>

## Credentials

The sources are checked in this order, and the first one found is used:

1. `-u user:pass` on the call.
   The value is visible in `ps`, so it suits manual use only.
2. `NXR_AUTH`, the base64 encoding of `user:pass`.
3. `NXR_USERNAME` and `NXR_PASSWORD`, which must be set together.

```bash
export NXR_USERNAME="my-login"
export NXR_PASSWORD="my-password"

# or a single value, which is usually easier in CI
export NXR_AUTH="$(printf '%s:%s' 'my-login' 'my-password' | base64)"
```

`nxr doctor [URL]` reports which source was used and does not print the value.

<details>
<summary>Named remotes and the alias file</summary>

Long URLs can be named in an optional TOML file.
The file is read only when `-R` names an alias from it, so a call without `-R` behaves the same on any machine:

```toml
# ~/.config/nxr/config.toml
[alias.releases]
url = "https://nexus.example.com/repository/raw-main"
user_env = "NEXUS_USERNAME"
pass_env = "NEXUS_PASSWORD"
```

```bash
nxr -R releases up dist/1.4.0/ 1.4.0/   # a relative path is joined to the alias URL
```

`NXR_CONFIG=/path/to/file` reads the file from another location.
The precedence rules are described in [name your remotes](docs/how-to/config.md).

</details>

## Other clients

The same protocol is also available as a terminal browser, a FUSE mount, a Rust library and a Node package.

| Component | Description | Install |
|:---|:---|:---|
| **Terminal browser** `nxr-tui` | tabs per server, repository tree at any depth, subtree downloads, live filter, saved presets | `cargo install nexus-raw-tui` |
| **Mount** `nxr-mount` | a raw repository as a local read-only directory, with lazy ranged reads over FUSE. It runs on Linux only. | included in the release archives |
| **Rust library** `nexus-raw-core` | the `Nxr` facade with the same transfers, events and rejections, in process | `cargo add nexus-raw-core` |
| **Node package** `@nexus-raw/nxr` | the CLI operations as promises, with the `nxr` command included | `npm install @nexus-raw/nxr` |

Server-to-server copies also run from the CLI: `nxr mirror` copies a version between repositories, `nxr clone` copies a whole repository, and `nxr audit` reports what is inconsistent inside one.

- TUI keys, configuration and headless mode: [TUI reference](docs/reference/tui.md).
- FUSE semantics, flags and the read-only limits: [the mount](docs/reference/mount.md).
- The facade, its layers and its guarantees: [Rust API](docs/reference/api.md).
- The promise surface and error shapes: [Node quickstart](docs/tutorials/node.md).

## Documentation

- The documentation site: <https://lipen.github.io/nexus-raw/>, with sources in [docs/](docs/).
- [Get started](docs/get-started.md) walks through one session against a local mock server.
- [Wire protocol](docs/reference/protocol.md) and [invariants](docs/reference/invariants.md) are written for people implementing another client.
- [Design](docs/explanation/design.md) records the decisions, and [conformance](docs/explanation/conformance.md) records the test evidence behind the guarantees above.
- Changes per release: [CHANGELOG.md](CHANGELOG.md).
- `just docs` serves the site locally with live reload.

## For agents

- [docs/how-to/agents.md](docs/how-to/agents.md) maps a task to a command and to the output that confirms it.
- The same content is available as a skill for Claude, omp and other agents: `scripts/install-skill.sh` from a checkout, or a copy from [`skills/nxr/`](skills/nxr).
- The machine-readable documentation index is [llms.txt](https://lipen.github.io/nexus-raw/llms.txt).
- Every error carries a `hint:` line, and `--json` produces NDJSON that a program can parse.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers the build, the tests, the commit rules, protocol changes and the release checklist.
[AGENTS.md](AGENTS.md) describes the repository layout and the rules for changing it.
`just check` runs the full gate, and `just demo` replays the recorded session against the local mock server.

## License

MIT, see [LICENSE](LICENSE).
