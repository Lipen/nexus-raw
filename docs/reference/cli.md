# CLI reference

`nxr` is curl for a Nexus raw repository.
Every invocation is self-sufficient: the URL comes from the command line and credentials from `-u` or the environment.
There is no config file and no profile.

## Credentials and URLs

Three ways to authenticate, in precedence order:

| Source | Form | Use it when |
|:-------|:-----|:------------|
| `-u user:pass` | plain `user:pass`, curl style | interactive one-offs — the argument is visible in `ps` |
| `NXR_AUTH` | base64 of `user:pass` | CI, where argv must stay clean |
| `NXR_USERNAME` + `NXR_PASSWORD` | the readable form | CI with masked variables |

`-u` wins over the environment.
`NXR_USERNAME` and `NXR_PASSWORD` must be set together — exactly one is a usage error, not a silent skip.
Credential values never appear in output, logs or `--json` events.

```bash
printf 'ci-bot:%s' "$TOKEN" | base64        # produce the NXR_AUTH value
export NXR_AUTH="Y2ktYm90OnRva2Vu"
nxr ls https://nexus.example.com/repository/raw-main/
```

The base URL is an ordinary argument of every call, normalized for you: `https://host/raw` and `https://host/raw/` are the same directory.
Only `http`/`https` with a host are accepted.
Shell aliases, wrapper scripts and CI variables are the way to shorten repeated URLs — `nxr` deliberately has no profile to store them.

```bash
alias nxr-main='nxr -u "$USER:$PASS" https://nexus.example.com/repository/raw-main/'
nxr-main up dist/1.4.0/
```

!!! warning "Credentials in argv"

    `nxr doctor` flags `-u` as resolved "from the -u flag" and the docs keep the env paths preferred for exactly one reason: an argv value shows up in `ps` and shell history.
    On a shared machine, use the environment.

## Global flags

All flags are global: they may appear before or after the subcommand.

| Flag | Default | Meaning |
|:-----|:--------|:--------|
| `-u, --user <USER:PASS>` | env | credentials for this call — beats `NXR_AUTH` and `NXR_USERNAME`/`NXR_PASSWORD` |
| `--workers <N>` | `8` | parallel artifact transfers |
| `--retry <N>` | `4` | attempts per HTTP request |
| `--connect-timeout-secs <N>` | `15` | TCP connect timeout |
| `--stall-secs <N>` | `30` | abort a transfer when no bytes arrive for this long |
| `--tls-insecure` | off | skip certificate verification |
| `--json` | off | machine output: one JSON object for simple commands, NDJSON events for transfers |
| `-q, --quiet` | off | only the final summary line |
| `-v, --verbose` | off | transfer starts, plan names, retry details |

## Primitives

### nxr get

`nxr get <URL> [-o FILE] [--continue]`

GET a URL to a file or stdout.
Without `-o` the body goes to stdout, untouched.
With `-o` the bytes stream into `<FILE>.part` and are renamed only after the transfer — a killed run never leaves a half-written file at the target path.

| Flag | Meaning |
|:-----|:--------|
| `-o, --out <FILE>` | output file — stdout when omitted |
| `--continue` | resume from an existing `<FILE>.part` through a `Range: bytes=N-` request |

```console
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app.zip -o app.zip
get: 10485760 bytes → app.zip
$ nxr get .../1.4.0/app.zip -o app.zip --continue
get: 10485760 bytes → app.zip (resumed from 4194304)
```

`--json`: `{"ok":true,"url":"…","out":"…","bytes":10485760,"resumed_from":0,"sha256":null}` — `sha256` appears when a sibling marker exists to check against.

### nxr put

`nxr put <URL> -f FILE [--sha]`

PUT a file as the URL's bytes, streamed, with `Content-Length`.

| Flag | Meaning |
|:-----|:--------|
| `-f, --file <FILE>` | the file to send |
| `--sha` | also PUT `<URL>.sha256` with a sha256sum-style marker, digest computed on the fly |

```console
$ nxr put https://nexus.example.com/repository/raw-main/1.4.0/app.zip -f app.zip --sha
put: 10485760 bytes + marker 9f86d081… → https://nexus.example.com/repository/raw-main/1.4.0/app.zip
```

`--json`: `{"ok":true,"url":"…","bytes":10485760,"marker":"9f86d081…"}` (`marker` is `null` without `--sha`).

### nxr head

`nxr head <URL>`

Status, size and content type.

```console
$ nxr head https://nexus.example.com/repository/raw-main/1.4.0/app.zip
head: 200 10485760 application/zip
```

`--json`: `{"url":"…","status":200,"size":10485760,"content_type":"application/zip"}`.

### nxr sha

`nxr sha <FILE|URL>`

The sha256 of a local file or a remote object, streamed.
Local files never touch the network.

```console
$ nxr sha app.zip
9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
$ nxr sha https://nexus.example.com/repository/raw-main/1.4.0/app.zip
9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
```

`--json`: `{"sha256":"9f86…","source":"app.zip"}`.

## Transfer

### nxr up

`nxr up <SRC_DIR> <DST_URL> [--manifest FILE|URL|-] [--no-sha] [--dry-run]`

Upload a local directory, verified and parallel.
The directory is scanned, every file becomes an artifact whose relative path is its name, and the symmetric diff against the server decides what moves.
Repeated names transfer again, identical ones are skipped.
An interrupted `up` is finished by repeating the same command.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | restrict the transfer to these names |
| `--no-sha` | skip marker generation and marker uploads — bytes only |
| `--dry-run` | print the plan without transferring anything |

Markers are on by default, in both directions:

- existing local `<name>.sha256` siblings are verified against the bytes and uploaded.
- missing local siblings are generated from the bytes, so a directory `up` has touched becomes self-complete.
- a local sibling that does not match its bytes refuses the run (`mismatch`, exit 1) before any request is made.

With `--manifest`, every manifest name must exist locally, or the run refuses with `missing` (exit 1).

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 2 to upload, 0 to download, 0 up to date
↑ app.zip ok
↑ pinned.xml ok
uploaded 2, downloaded 0, skipped 0
up: 2 sent, 0 fetched, 0 skipped
```

The event stream ends with an `uploaded/downloaded/skipped` line, and the command adds its own `up:` summary — `-q` keeps only those two lines, and their relative order is not fixed because the renderer is concurrent.

`--dry-run` prints one line per name:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --dry-run
upload app.zip
upload pinned.xml
```

### nxr down

`nxr down <SRC_URL> <DST_DIR> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--continue]`

Download a remote directory into a local one — the mirror of `up`.
Every artifact is streamed into a stable part file, hashed on the fly, checked against the remote marker when one exists, then renamed into place.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | enumeration source: a local file, a URL, or `-` for stdin |
| `--name <NAME>` | one explicit name, repeat as needed |
| `--ls` | best-effort enumeration through the server search API |
| `--continue` | resume interrupted downloads from their part files |

`down` must know *what* to fetch, and Nexus raw has no guaranteed directory listing.
Without flags it tries the recommended convention first: a `manifest.json` at the directory URL.
If that is absent, the run refuses with `cannot enumerate` (exit 1) and a hint naming the three explicit sources.
`--manifest` and repeatable `--name` are exact.
`--ls` is best-effort and depends on the server's search API.

```console
$ nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ --continue
plan: 0 to upload, 2 to download, 0 up to date
↓ app.zip ok
↓ pinned.xml ok
uploaded 0, downloaded 2, skipped 0
down: 0 sent, 2 fetched, 0 skipped
```

Every successful download writes its local `<name>.sha256` sibling, computed from the received bytes — a downloaded directory verifies offline even when the server never had a marker.
A digest disagreement with the remote sibling refuses the download and deletes the part file, so nothing divergent is ever written.
Part files are named `.nxr-part-<hash>` and are stable per artifact name, so `--continue` picks up exactly where the killed run stopped.
Without `--continue` the transfer starts over.

### nxr ls

`nxr ls <URL> [--assets]`

List versions under a repository or group URL, or objects under a directory URL.
Both go through the Nexus search API — **experimental**, behavior depends on the server release.

| Flag | Meaning |
|:-----|:--------|
| `--assets` | list the object names under a directory URL instead of versions |

```console
$ nxr ls https://nexus.example.com/repository/raw-main/
1.3.0
1.4.0
1.5.0
$ nxr ls https://nexus.example.com/repository/raw-main/1.4.0/ --assets
app.zip
pinned.xml
```

`--json` prints one `{"version":"…"}` or `{"name":"…"}` object per line.

## Layout helpers

### nxr channel get

`nxr channel get <URL>`

Print the token of a channel ref — a file at any name whose content is one version token.

```console
$ nxr channel get https://nexus.example.com/repository/raw-main/latest
1.5.0
$ nxr channel get https://nexus.example.com/repository/raw-main/stable
unset
```

`--json`: `{"url":"…","token":"1.5.0"}` (`token` is `null` when unset, with exit 0 either way).

### nxr channel set

`nxr channel set <URL> <TOKEN> [--if-forward]`

Write the token.
`latest`, `nightly`, `stable`, `prod` — a channel is any name, `nxr` has no reserved list.

| Flag | Meaning |
|:-----|:--------|
| `--if-forward` | keep the current token when it already compares ≥ the new one (dotted-numeric order) |

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/latest 1.5.0 --if-forward
channel: set …/latest → 1.5.0
$ nxr channel set https://nexus.example.com/repository/raw-main/latest 1.4.0 --if-forward
channel: kept …/latest at 1.5.0 (forward-only)
```

`--json`: `{"url":"…","outcome":"written","token":"1.5.0","from":null}` or `{"url":"…","outcome":"skipped","current":"1.5.0"}`.

### nxr verify

`nxr verify <DIR> [--manifest FILE|-]`

Check local bytes, markers and digests.
No network — the run is entirely offline.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|->` | check exactly these names instead of the whole directory |

Every checked name must be complete: bytes present, sibling present, digest matching.
Markerless, broken or missing names are reported and the exit is 1.

```console
$ nxr verify dist/1.4.0/
verify: 2 ok, FAILED: none
uploaded 0, downloaded 0, skipped 2
$ nxr verify dist/1.4.0/
verify: 1 ok, FAILED: app.zip
error: incomplete: app.zip
hint: rerun the same command; finished names are skipped and the rest is retried
```

### nxr doctor

`nxr doctor [URL]`

Diagnose the setup before a pipeline runs.
Local checks always execute: credentials (resolved presence and source, never values), TLS mode, worker and timeout settings.
With a URL: one `HEAD` probe for reachability.

```console
$ nxr doctor https://nexus.example.com/repository/raw-main/
doctor:
  [  ok  ] credentials: resolved from NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] probe: HEAD … → HTTP 200
  all checks passed
```

Exit codes follow the failure class: a local gap (for example no credentials anywhere) is misuse and exits 2, a failed probe is transport and exits 3.
`doctor` always renders the human report, `--json` or not.

## Output

Human output goes to stdout, errors and `hint:` lines to stderr.
`--json` switches stdout to machine form:

- primitives (`get` with `-o`, `put`, `head`, `sha`) and `channel` print **one JSON object**.
- transfers (`up`, `down`) and `verify` print an **NDJSON event stream** (`plan`, `artifact`, `retrying`, `summary`).
- the body of `get` without `-o` is raw bytes on stdout — do not mix it with `--json`.

`-q` trims to the final summary line, `-v` adds plan names and transfer starts.

## Exit codes

| Code | Class | Covers |
|:-----|:------|:-------|
| `0` | ok | transferred, converged, or a successful check |
| `1` | data | `mismatch`, `incomplete`, `missing`, `cannot enumerate` |
| `2` | misuse | bad flags, unsafe names, half-set credentials |
| `3` | transport | network, auth, TLS, unexpected statuses — after retries |

Every error prints a `hint:` line on stderr.
Details: [errors and exit codes](errors.md).
