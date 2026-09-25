# CLI reference

The `nxr` binary: six commands, one set of global flags.

## Global flags

| Flag | Default | Meaning |
|:-----|:--------|:--------|
| `--profile <name>` | — | select a profile from the config file |
| `--base <url>` | — | repository base URL, wins over the profile |
| `--workers <n>` | `8` | parallel artifact transfers, `1..=64` |
| `--retry <n>` | `4` | attempts per HTTP request, `1..=32` |
| `--connect-timeout-secs <n>` | `15` | TCP connect timeout |
| `--stall-secs <n>` | `30` | abort a transfer with no bytes for this long |
| `--tls-insecure` | off | skip certificate verification |
| `--config <path>` | XDG | config file location, overrides `$NXR_CONFIG` |
| `--no-config` | off | ignore the config entirely |
| `--json` | off | NDJSON events on stdout |
| `-q`, `--quiet` | off | only the final summary |
| `-v`, `--verbose` | off | transfer starts and retry details |

`verify` runs offline: it needs neither `--profile` nor `--base`.

## nxr up

Publish a version directory: claim (drift check) → diff → PUT bytes → PUT markers, in parallel workers.

```bash
nxr up --profile main --dir dist/1.4.0 [--names claim-other.json] [--dry-run]
```

| Flag | Meaning |
|:-----|:--------|
| `--dir <dir>` | the version directory; must exist |
| `--names <file>` | claim file, default `<dir>/claim.json` |
| `--dry-run` | diff and print the plan; write nothing |

## nxr down

Fetch a version, or a subset, into a directory; resume is always on.

```bash
nxr down --profile main --dir vendor/ (--version 1.4.0 | --pointer latest) [--only NAME]... [--names claim.json]
```

| Flag | Meaning |
|:-----|:--------|
| `--dir <dir>` | target directory; created when missing |
| `--version <v>` | exact version to fetch |
| `--pointer <name>` | `latest` or `nightly`; resolves to a version |
| `--only <name>` | restrict to these claim artifacts, repeatable |
| `--names <file>` | a claim file whose artifacts join the `--only` filter |

## nxr verify

Local check only: every claim artifact must be Complete — bytes, marker, matching digest.

```bash
nxr verify --dir dist/1.4.0 [--names claim-other.json]
```

No profile, no base, no network.

## nxr diff

Print the plan against the server, symmetric for up and down.

```bash
nxr diff --profile main --dir dist/1.4.0 [--names claim-other.json]
```

```console
$ nxr diff --profile main --dir dist/1.4.0
plan: 1 to upload, 0 to download, 2 up to date
```

## nxr ls

With `--version`: per-name remote states from the version's claim.
Without: the repository's version list via the Nexus search API — experimental, depends on the server release.

```bash
nxr ls --profile main --version 1.4.0
nxr ls --profile main
```

```console
$ nxr ls --profile main --version 1.4.0
name        state       size
app.zip     complete    8
pinned.xml  markerless  7
```

## nxr point

Atomically move a pointer to a version; `--if-newer` moves only forward in version order.

```bash
nxr point latest 1.4.0 [--if-newer] --profile main
```

```console
$ nxr point latest 1.5.0 --profile main
pointed latest at 1.5.0 (was 1.4.0)
$ nxr point latest 1.4.0 --if-newer --profile main
skipped: latest already at 1.5.0
```

## Exit codes

| Code | Class |
|:-----|:------|
| `0` | converged |
| `1` | data: mismatch, incomplete, claim drift, missing |
| `2` | misuse: flags, config, unsafe names |
| `3` | transport: network, auth, TLS, unexpected statuses |

Details: [errors and exit codes](errors.md).
