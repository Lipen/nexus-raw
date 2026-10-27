# nxr

`nxr` is curl for a Sonatype Nexus raw repository: the HTTP primitives, plus parallel directory transfers with sha256 markers and resume.

It is one static binary, and the server needs no additional component.
The URL is an argument, credentials come from the call or from the environment, and the repository contents are the only state.

- Names are arbitrary relative paths, and the tool imposes no repository layout.
- A directory transfer compares the local tree with the server before copying and moves only the difference.
  An interrupted transfer resumes.
- A divergent complete artifact is refused, never overwritten.
- Exit codes: `0` ok, `1` data, `2` misuse, `3` transport.
  Every error carries a `hint:` line.

## Install

```bash
cargo binstall nexus-raw     # a prebuilt static archive
cargo install nexus-raw      # from source, Rust 1.88 or newer
```

Homebrew on macOS and Linux, or npm with the `nxr` command included:

```bash
brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
brew install lipen/nexus-raw/nexus-raw

npm install @nexus-raw/nxr
```

Prebuilt archives for Linux x64 and arm64, macOS Intel and Apple silicon, and Windows x64 are on [GitHub Releases](https://github.com/Lipen/nexus-raw/releases/latest), with a `SHA256SUMS` next to them.
The image `ghcr.io/lipen/nexus-raw:latest` is the same binary on a distroless base.
Check the install with `nxr --version`.

## Quick start

```bash
BASE=https://nexus.example.com/repository/raw-main

nxr up dist/1.4.0/ "$BASE/1.4.0/"                   # publish, with generated markers
nxr channel set "$BASE/latest" 1.4.0 --if-forward   # write the channel token
V=$(nxr channel get "$BASE/latest")
printf 'app-1.4.0.zip\nmanifest.json\n' > names.txt
nxr down "$BASE/$V/" vendor/prebuilt --names-from names.txt
nxr verify vendor/prebuilt                          # offline
```

```console
$ nxr up dist/1.4.0/ "$BASE/1.4.0/"
plan: 4 to upload, 0 to download, 0 up to date
↑ app-1.4.0.zip ok
↑ bom/linux-x86_64.json ok
↑ manifest.json ok
↑ pinned.xml ok
uploaded 4, downloaded 0, skipped 0
```

The example above is one common pipeline: a release directory and a channel that names its current version.
Both are conventions of the caller: `version.json`, `latest` and `nightly` are ordinary names, and `nxr channel` only stores and compares a token.
Run the same command again and nothing transfers, because the server already holds byte-identical copies with equal markers.
If a transfer is interrupted, run it again: what landed is skipped, the rest continues from a `Range` request.

## Commands

The ones a release pipeline needs:

| Command | Does |
|:--------|:-----|
| `nxr up <DIR> <URL>` | publish a directory: scan, diff, PUT bytes and markers in parallel |
| `nxr down <URL> <DIR>` | fetch named artifacts, verified and resumable |
| `nxr verify <DIR>` | check local bytes, markers and digests, offline |
| `nxr diff <DIR> <URL>` | report the delta, write nothing, exit 1 on a difference |
| `nxr ls <URL>` | list a directory URL at any depth, `--assets` for flat names |
| `nxr channel get\|set` | read a channel token, or write one with `--if-forward` |
| `nxr doctor [URL]` | credentials, TLS, settings and reachability, secrets never printed |
| `nxr get\|put\|head\|sha` | the curl primitives, with retries, stall detection and digests |

<details>
<summary>The rest of the CLI</summary>

| Command | Does |
|:--------|:-----|
| `nxr clone <URL> <DIR>` | clone a whole repository, with an optional fact cache that skips re-hashing unchanged files |
| `nxr audit <URL>` | integrity report: orphans, dangling markers, disagreeing facts. `--bytes` verifies every object, `--sample N` a deterministic sample |
| `nxr mirror <SRC> <DST>` | copy a set of names between two repositories |
| `nxr mv <SRC> <DST>` | mirror, then delete at the source once the pour converged |
| `nxr rm <URL>` | delete the listed names: a plan by default, `--yes` deletes markers first, then bytes |
| `nxr point <URL>` | delete one pointer file, idempotently |
| `nxr service repos\|status\|repo\|assets\|eula <URL>` | the server's REST API: repositories, liveness, settings, the search snapshot |
| `nxr complete <shell>` | shell completion for bash, zsh, fish and PowerShell |

Every command takes `--workers N`, `--retry N`, `--connect-timeout-secs`, `--stall-secs`, `--tls-insecure`, `--ca-bundle FILE`, `--json`, `-q` and `-v`.
`nxr <command> --help` is always current for the installed binary.

</details>

## Credentials

Sources are checked in this order, first hit wins:

1. `-u user:pass` on the call, visible in `ps`, so not for scripts.
2. `NXR_AUTH`, the base64 of `user:pass`.
3. `NXR_USERNAME` and `NXR_PASSWORD`, set together.

`-R <alias>` names a remote from `~/.config/nxr/config.toml`, which is read only when the flag names an alias in it.

## More

- The documentation site: <https://lipen.github.io/nexus-raw/>.
  Start with [get started](https://lipen.github.io/nexus-raw/get-started/) or the [CLI reference](https://lipen.github.io/nexus-raw/reference/cli/).
- The protocol this CLI implements: the [`nexus-raw-core`](https://crates.io/crates/nexus-raw-core) crate docs.
- A terminal browser over the same stores: [`nxr-tui`](https://crates.io/crates/nexus-raw-tui).
- The repository: <https://github.com/Lipen/nexus-raw>.
- License: MIT.
