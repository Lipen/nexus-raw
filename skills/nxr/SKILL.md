---
name: nxr
description: Transfer artifacts to and from a Sonatype Nexus raw repository with the nxr CLI (curl-shaped, URL in argv, credentials from -u or env, no config file). Use when publishing, downloading, cloning, mirroring, renaming or deleting version directories, or verifying and auditing Nexus raw artifacts, or when a task mentions nxr, Nexus raw, .sha256 marker siblings, or channel and pointer refs.
license: MIT
---

# nxr: Nexus raw transfers from the command line

`nxr` is curl for a Sonatype Nexus raw repository.
A URL goes in argv, credentials come from `-u` or the environment, and no config file is read unless `-R` names an alias.
It moves whole directories with resumable, digest-verified transfers, and it writes a `.sha256` sibling marker next to every artifact it uploads.

Reach for it whenever a task needs to publish, fetch, mirror, rename, delete, verify or audit artifacts on a Nexus raw repository.

## Ground rules

Completion is bytes plus the sha sibling with the digest matching.
A divergent complete artifact is never overwritten, so a rerun is always safe.

- `up` writes markers by default and generates the missing local sibling, and `--no-sha` is the explicit opt-out.
- The marker of a name is always PUT after that name's bytes, and the order between names is free.
- Names are relative paths of `[A-Za-z0-9._-]` segments, and only the `.sha256` and `.part` suffixes and a `.nxr`-prefixed segment are reserved.
  A raw repository still holds foreign files with spaces, parens and unicode: `ls` and `down` handle them, and the alphabet constrains only what nxr itself writes.
- The local scan refuses a symlink or any other non-regular entry: a link is never followed out of the source.
- `version.json`, `latest` and `nightly` are ordinary names with no protocol meaning.
  A channel is any token file, and dotted-numeric order is the only comparison nxr imposes (`--if-forward`).

## Credentials

They resolve in this order: `-u user:pass`, then `NXR_AUTH` (base64 `user:pass`), then `NXR_USERNAME` plus `NXR_PASSWORD`.
`-u` puts the secret in argv, so prefer the environment variables in CI, and let `nxr doctor` flag the exposure.
Values never appear in logs, `--json` output or error messages.

TLS verifies by default.
A private CA is `--ca-bundle FILE` (or `NXR_CA_BUNDLE`), a PEM bundle of additional roots. `--tls-insecure` is the last resort and conflicts with it.

## The command map

| Intent | Command |
|:--|:--|
| is the server alive and writable? | `nxr service status <url>` |
| which repository is this URL in? | `nxr service repo <url>` |
| every asset of a repo, any format | `nxr service assets <repo-url>` |
| the server refuses writes with 403 | `nxr service eula <url> --accept` |
| list a directory | `nxr ls <dir-url>` and `--assets` for flat names |
| fetch one file | `nxr get <url> -o <file>` |
| fetch one file with the marker and resume | `nxr down <file-url> <dst-dir>` |
| fetch one file through 8 ranges | `nxr get <url> -o <file> --segments 8` |
| fetch one byte span | `nxr get <url> --range 100-199` |
| publish a subtree | `nxr up <src-dir> <dir-url> --prefix <dir>/` (the whole tree is scanned first) |
| stream one file to stdout | `nxr get <url>` |
| fetch a directory | `nxr down <dir-url> <dst-dir> --names-from list.txt` |
| clone a whole repository for backup | `nxr clone <repo-url> <dst-dir>` |
| publish a directory | `nxr up <src-dir> <dir-url>` |
| what differs locally versus remote | `nxr diff <dir> <dir-url> --names-from list.txt` |
| copy a version between repositories | `nxr mirror <src-url> <dst-url> --names-from list.txt` |
| move a version, then delete the source | `nxr mv <src-url> <dst-url> --names-from list.txt` |
| delete names | `nxr rm <dir-url> --names-from list.txt` plus `--yes` |
| delete a pointer file | `nxr point <ptr-url>` |
| read a channel | `nxr channel get <url>` |
| write a channel, forward only | `nxr channel set <url> <token> --if-forward` |
| verify local bytes with no network | `nxr verify <dir>` |
| audit a repository for integrity | `nxr audit <repo-url>` |
| check credentials, TLS and reachability | `nxr doctor <url>` |

If `nxr` is not on PATH, install it with `cargo install nexus-raw`, `npm install -g @nexus-raw/nxr`, or a prebuilt binary from the releases page.

## The machine contract

- Exit codes: `0` ok, `1` data (a 404, a digest miss, `cannot enumerate`), `2` misuse (a bad invocation), `3` transport (network, auth, 5xx).
- Every error prints one `hint:` line on stderr saying what to check next.
  The hint ends with `server says: <first body line>` when the server sent one.
- Enumeration needs a source: a repeatable `--name`, a `--names-from FILE` (`-` reads stdin), or `--ls` for the server search API.
  Without one, `down`, `diff`, `mirror`, `mv` and `rm` refuse with `cannot enumerate`.
  `clone` enumerates the repository itself through the search API and refuses with the same error when the API is absent.
- `rm` and `mv` are plan-by-default: they print what they would do and touch nothing until `--yes`.
- Redirects are never followed, so a 301 surfaces as a plain transport error (exit 3).
- TLS verification is on, and `--tls-insecure` is the only way off.
- `--json` is machine output, and the shape depends on the command:
  - `up`, `down`, `mirror` and `rm` print one NDJSON event per line and end on a `summary` event carrying `uploaded`, `downloaded`, `skipped`, `removed`, `failed` and `failures`.
    `failures` is `[{code, name}]` with the exit class (1 data, 2 misuse, 3 transport), in the same order as `failed`.
  - `put` and `get -o` print `artifact` events and then one result object shaped like `{"ok":true,…}`.
  - `verify` prints a `summary` event and, when names failed, a closing `error` object.
  - `audit` prints one `finding` event per finding and then a `summary` event carrying `audited` and `findings`.
  - `ls` prints one object per entry, then a `{"event":"hidden","count":N}` line when it hid markers.
  - `doctor` prints one object per check.
  - `head`, `sha`, `channel`, `point` and `service` print a single object.

## Recipes

Step-by-step tasks live in [references/recipes.md](references/recipes.md): publish a release, resume a killed transfer, mirror between servers, promote a channel, delete a version safely, audit in CI and gate a download on a channel.

## Server facts that are not bugs

- Nexus CE 3.79 and later answers 403 to every PUT until the EULA is accepted: run `nxr service eula <url> --accept`, which needs nx-admin.
- `/service/rest/v1/status` answers 200 with an empty body there, so `nxr service status` is the liveness check.
- The `/v1/repositories` collection is trimmed even for an admin, and full settings need `nxr service repo <url> --detail`, which also needs nx-admin.
- Anonymous and authenticated users see different repository lists, and a 403 on `status/check` means "not an admin", not "the server is down".
- The search index lags writes: right after an `up`, `ls --assets` and `service assets` can look short for a few seconds.
  A repository filled by a foreign publisher can be partially indexed, so confirm a specific name with `head` or `get` instead of trusting a listing.
- `-o` must be a real path: nxr writes a neighbouring `<name>.part` and renames it once the bytes are complete, which is exactly the resume scheme.
- npm repositories: browse them with `service assets`, but fetch packages with the npm client, not `nxr get`.
- A `/-/` path segment is the npm protocol (dist-tags, tarballs), not part of the nxr protocol.

The full list, with the reasoning, is in [references/quirks.md](references/quirks.md).

## When not to use nxr

- Talking to a non-raw Nexus format for transfers (maven, npm, pypi, docker): use the client for that format.
  `nxr service assets` can still browse any format.
- Local filesystem work: `verify` is the only offline command.
- Anything that wants a manifest, a lockfile or a config file: nxr deliberately has none, and the tree layout is the caller's.

## More

- `nxr <command> --help` is the authoritative offline flag reference.
- Docs: the [CLI reference](https://lipen.github.io/nexus-raw/reference/cli/), the [error taxonomy](https://lipen.github.io/nexus-raw/reference/errors/), the [wire protocol](https://lipen.github.io/nexus-raw/reference/protocol/).
- A one-page agent index lives at <https://lipen.github.io/nexus-raw/llms.txt>.
