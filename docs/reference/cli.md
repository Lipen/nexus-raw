# CLI reference

`nxr` is curl for a Nexus raw repository: primitives with retries and TLS on, verified directory transfers, channel refs and manifests.
Every invocation is self-sufficient: the URL comes from the command line and credentials from `-u` or the environment.
There is no config file, no profile and no state directory, on purpose: a tool that publishes software must be reproducible from its own command line, and any persistent setting would make two runs of "the same command" different.
Shell aliases, wrapper scripts and CI variables are the supported way to shorten repeated invocations.

All transcripts on this page are real output. They were captured against a live Sonatype Nexus Repository, with the deterministic failure mock standing in for scenarios a healthy server cannot produce on demand.
Hostnames are neutralized.
Exit codes are quoted as observed.

## Credentials and URLs

Three ways to authenticate, in precedence order:

| Source | Form | Use it when |
|:-------|:-----|:------------|
| `-u user:pass` | plain `user:pass`, curl style | interactive one-offs (the argument is visible in `ps`) |
| `NXR_AUTH` | base64 of `user:pass` | CI, where argv must stay clean |
| `NXR_USERNAME` + `NXR_PASSWORD` | the readable form | CI with masked variables |

`-u` wins over the environment.
`NXR_USERNAME` and `NXR_PASSWORD` must be set together. Exactly one alone is a usage error instead of a silent skip.
Credential values never appear in output, logs or `--json` events.

```bash
printf 'ci-bot:%s' "$TOKEN" | base64        # produce the NXR_AUTH value
export NXR_AUTH="Y2ktYm90OnRva2Vu"
nxr ls https://nexus.example.com/repository/raw-main/
```

`doctor` proves which path resolved without printing the secret:

```console
$ nxr doctor https://nexus.example.com/repository/raw-main/
doctor:
  [  ok  ] credentials: resolved from NXR_USERNAME + NXR_PASSWORD
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] probe: HEAD https://nexus.example.com/repository/raw-main/ → HTTP 400
  all checks passed
$ echo $?
0
```

The base URL is an ordinary argument of every call, normalized for you: `https://host/raw` and `https://host/raw/` are the same directory.
Only `http`/`https` with a host are accepted.
To shorten repeated invocations, wrap the whole command in a shell function that keeps URL composition in one place:

```bash
rel() { nxr up "dist/$1/" "https://nexus.example.com/repository/raw-main/$1/"; }
rel 1.4.0
```

!!! warning "Credentials in argv"

    `nxr doctor` flags `-u` as resolved "from the -u flag" and the docs keep the env paths preferred for exactly one reason: an argv value shows up in `ps` and shell history.
    On a shared machine, use the environment.

## Global flags

All flags are global: they may appear before or after the subcommand.

| Flag | Default | Meaning |
|:-----|:--------|:--------|
| `-u, --user <USER:PASS>` | env | credentials for this call, takes precedence over `NXR_AUTH` and `NXR_USERNAME`/`NXR_PASSWORD` |
| `--workers <N>` | `8` | parallel artifact transfers, accepted range `1..=64` |
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
Without `-o` the body goes to stdout untouched, in a single attempt: a retry after the body started would duplicate bytes.
With `-o` the bytes stream into `<FILE>.part` and are renamed only after the transfer, so a killed run never leaves a half-written file at the target path.

| Flag | Meaning |
|:-----|:--------|
| `-o, --out <FILE>` | output file, stdout when omitted |
| `--continue` | resume from an existing `<FILE>.part` through a `Range: bytes=N-` request |

```console
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
koala app payload v1 for docs capture
$ echo $?
0
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip
get: 38 bytes → app.zip
$ echo $?
0
```

Resuming from a 20-byte part appends through a Range request and renames when complete:

```console
$ head -c 20 dist/1.4.0/app-1.4.0.zip > app.zip.part
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip --continue
get: 38 bytes → app.zip (resumed from 20)
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | body delivered |
| `2` | misuse: a non-http URL |
| `3` | 404, auth failure, transport exhaustion: see [errors](errors.md#captured-transcripts) |

Recipes:

```bash
# Preview a BOM without touching disk.
nxr get https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json | jq .

# Finish a download a flaky link killed twice.
nxr get "$URL/app.zip" -o app.zip --continue
```

`--json`: `{"bytes":38,"ok":true,"out":"app.zip","resumed_from":0,"sha256":"54e0bee9…","url":"…"}`, where `sha256` is the digest of the written file and `resumed_from` the part offset.
Without `-o` the stdout body is raw bytes. Do not combine it with `--json`.

### nxr put

`nxr put <URL> -f FILE [--sha]`

PUT a file as the URL's bytes, streamed, with `Content-Length`.
PUTs are idempotent and replay whole after a broken attempt.

| Flag | Meaning |
|:-----|:--------|
| `-f, --file <FILE>` | the file to send |
| `--sha` | also PUT `<URL>.sha256` with a sha256sum-style marker, digest computed on the fly |

```console
$ nxr put https://nexus.example.com/repository/raw-main/1.4.1/app-1.4.1.zip -f app-1.4.1.zip --sha
put: 15 bytes + marker 77ce85a7fc76a62fa53e50e8e352b0bd39e5806341c993d1bc6622dbd4ceda42 → https://nexus.example.com/repository/raw-main/1.4.1/app-1.4.1.zip
$ echo $?
0
$ nxr put https://nexus.example.com/flaky-c/app.bin -f mock.bin
put: 13 bytes → https://nexus.example.com/flaky-c/app.bin (no marker)
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | bytes (and marker, when asked) stored |
| `2` | misuse: missing input file |
| `3` | auth failure, 5xx after retries, unexpected status |

Recipes:

```bash
# Publish one hotfixed artifact into an existing version directory.
nxr put "$BASE/1.4.0/app.zip" -f fixed/app.zip --sha

# Park a build log next to the release.
nxr put "$BASE/1.4.0/build.log" -f build.log
```

`--json`: `{"bytes":15,"marker":"77ce85a7…","ok":true,"url":"…"}`, where `marker` is `null` without `--sha`.

### nxr head

`nxr head <URL>`

Status, size and content type.
`head` never fails on a status: the answer *is* the result, which makes it the safe preflight in scripts.

```console
$ nxr head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
head: 200 38 application/zip
$ echo $?
0
$ nxr head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -u ci-bot:wrong-pass
head: 401 - -
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | a status was received: any status, including 401 and 404 |
| `2` | misuse: a non-http URL |
| `3` | the connection itself failed |

Recipes:

```bash
# Gate a pipeline on the artifact existing before downloading.
test "$(nxr head "$BASE/1.4.0/app.zip" | awk '{print $2}')" = 200

# Compare sizes before and after a mirror copy.
nxr head "$BASE/1.4.0/app.zip"
```

`--json`: `{"content_type":"application/zip","size":38,"status":200,"url":"…"}`, where `size` and `content_type` are `null` on non-2xx.

### nxr sha

`nxr sha <FILE|URL>`

The sha256 of a local file or a remote object, streamed.
Local files never touch the network, which makes this the offline test of the digest pipeline.

```console
$ nxr sha dist/1.4.0/app-1.4.0.zip
54e0bee99b80197dc68472b6b3b7c67584df16584a5f27db0cc8ba479f2bc1d9
$ nxr sha https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
54e0bee99b80197dc68472b6b3b7c67584df16584a5f27db0cc8ba479f2bc1d9
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | digest printed |
| `2` | misuse: missing local file |
| `3` | transport failure on a URL source |

Recipes:

```bash
# Reproduce the marker line nxr would upload.
printf '%s  %s\n' "$(nxr sha app.zip)" app.zip

# Check a downloaded copy against the published digest.
test "$(nxr sha app.zip)" = "$(nxr get "$URL/app.zip.sha256" | cut -d' ' -f1)"
```

`--json`: `{"sha256":"54e0bee9…","source":"dist/1.4.0/app-1.4.0.zip"}`.

## Transfer

### nxr up

`nxr up <SRC_DIR> <DST_URL> [--manifest FILE|URL|-] [--no-sha] [--dry-run]`

Upload a local directory, verified and parallel.
The directory is scanned, every file becomes an artifact whose relative path is its name, and the [symmetric diff](protocol.md#the-symmetric-diff) against the server decides what moves.
Repeated names transfer again, identical ones are skipped, and an interrupted `up` is finished by repeating the same command.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | restrict the transfer to these names, all of which must exist locally |
| `--no-sha` | skip marker generation and marker uploads, bytes only |
| `--dry-run` | print the plan without transferring anything |

Markers are on by default, in both directions:

- existing local `<name>.sha256` siblings are verified against the bytes and uploaded.
- missing local siblings are generated from the bytes, so a directory `up` has touched becomes self-complete.
- a local sibling that does not match its bytes refuses the run (`mismatch`, exit 1) before any byte moves.

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 3 to upload, 0 to download, 0 up to date
↑ app-1.4.0.zip ok
↑ bom/linux-x86_64.json ok
↑ pinned.xml ok
up: 3 sent, 0 fetched, 0 skipped
uploaded 3, downloaded 0, skipped 0
$ echo $?
0
```

The event stream ends with an `uploaded/downloaded/skipped` line, and the command adds its own `up:` summary. Their relative order is not fixed because the renderer is concurrent.
Re-running converges: the plan empties and every name reports `skipped`.

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 0 to upload, 0 to download, 3 up to date
up: 0 sent, 0 fetched, 3 skipped
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ pinned.xml skipped
uploaded 0, downloaded 0, skipped 3
$ echo $?
0
```

A new file uploads alone, and `--dry-run` shows the plan of exactly that, with every name marked `skip` or `upload`:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --dry-run
skip app-1.4.0.zip
skip pinned.xml
upload release-notes.txt
$ echo $?
0
```

With `--json` the plan is one object per line:

```json
{"action":"skip","name":"first.txt"}
{"action":"upload","name":"second.txt","size":12}
```

| Exit | When |
|:----:|:-----|
| `0` | plan executed, remote converged |
| `1` | data refusal: `mismatch`, `missing` (a `--manifest` name absent locally), `incomplete` |
| `2` | misuse: not a directory, empty directory, unsafe name |
| `3` | transport or auth failure: the `failed:` list names what did not land |

Recipes:

```bash
# Publish a release directory with markers, the standard publish path.
nxr up "dist/$VERSION/" "$BASE/$VERSION/"

# Ship bytes only when a legacy consumer rejects sibling files.
nxr up dist/1.4.0/ "$BASE/1.4.0/" --no-sha

# Rehearse in CI before touching the server.
nxr up "dist/$VERSION/" "$BASE/$VERSION/" --dry-run
```

`--json` prints the NDJSON stream: `plan`, `artifact`, optional `retrying`, then `summary` (see [output](#output)).

### nxr down

`nxr down <SRC_URL> <DST_DIR> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--continue]`

Download a remote directory into a local one, the mirror of `up`.
Every artifact is streamed into a stable part file, hashed on the fly, checked against the remote marker when one exists, then renamed into place and given a local sibling computed from the received bytes.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | enumeration source: a local file, a URL, or `-` for stdin |
| `--name <NAME>` | one explicit name, repeat as needed |
| `--ls` | best-effort enumeration through the server search API |
| `--continue` | resume interrupted downloads from their part files |

`down` must know *what* to fetch, and Nexus raw has no guaranteed directory listing.
Without flags it tries the recommended convention first: a `manifest.json` at the directory URL.
If that is absent the run refuses with `cannot enumerate` (exit 1) and a hint naming the three explicit sources (see [the refusal transcript](errors.md#enumeration-refusal-exit-1)).

```console
$ nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ \
    --name app-1.4.0.zip --name pinned.xml
plan: 0 to upload, 2 to download, 0 up to date
↓ app-1.4.0.zip ok
↓ pinned.xml ok
down: 0 sent, 2 fetched, 0 skipped
uploaded 0, downloaded 2, skipped 0
$ echo $?
0
```

A manifest from stdin keeps the name list under the consumer's control:

```console
$ printf '{"artifacts":["app-1.4.0.zip","pinned.xml"]}' \
    | nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ --manifest -
plan: 0 to upload, 2 to download, 0 up to date
↓ pinned.xml ok
↓ app-1.4.0.zip ok
down: 0 sent, 2 fetched, 0 skipped
uploaded 0, downloaded 2, skipped 0
$ echo $?
0
```

The downloaded directory verifies offline, because every fetched name gets a local sibling:

```console
$ cat vendor/app/app-1.4.0.zip.sha256
54e0bee99b80197dc68472b6b3b7c67584df16584a5f27db0cc8ba479f2bc1d9  app-1.4.0.zip
$ nxr verify vendor/app/
verify: 2 ok, FAILED: none
uploaded 0, downloaded 0, skipped 2
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | plan executed, local copies complete |
| `1` | data refusal: `cannot enumerate`, `missing`, `mismatch` with the remote sibling |
| `2` | misuse: unsafe `--name`, unwritable target |
| `3` | transport or auth failure |

Recipes:

```bash
# Pin a deployment: fetch exactly the manifest the publisher signed off.
nxr down "$BASE/1.4.0/" deploy/ --manifest https://nexus.example.com/repository/raw-main/1.4.0/manifest.json

# Resume a CI cache restore that died mid-run.
nxr down "$BASE/1.4.0/" cache/ --manifest cache-manifest.json --continue

# Pull one config file without enumerating anything.
nxr down "$BASE/1.4.0/" . --name pinned.xml
```

Part files are named `.nxr-part-<hash>` and are stable per artifact name, so `--continue` picks up exactly where the killed run stopped.
Without `--continue` the transfer starts over.
A digest disagreement with the remote sibling refuses the download and deletes the part file, so nothing divergent is ever written.

### nxr ls (experimental)

`nxr ls <URL> [--assets]`

List versions under a repository or group URL, or objects under a directory URL.
Both go through the Nexus search API. This path is **experimental**, and behavior depends on the server release.

| Flag | Meaning |
|:-----|:--------|
| `--assets` | list the object names under a directory URL instead of versions |

At a repository root the command lists the first path segment of every indexed asset, which on a well-formed store is the version directory.
Loose files at the root appear as names too, which this live capture shows plainly:

```console
$ nxr ls https://nexus.example.com/repository/raw-main/
_docs
_docs-ref
latest
nxr-ct-test.bin
p1.bin
p1.bin.sha256
v0.1
$ echo $?
0
```

Measured caveat from the same live server: for a *group* URL the search request filters by `group`, and on this Nexus release that parameter matches Maven coordinates rather than raw path prefixes. A group-scoped `ls` therefore returns an empty list with exit 0.
Treat `ls` as a convenience only. `down` takes its names from manifests instead of from listings.
The exact enumeration guarantees live in [the protocol page](protocol.md#enumeration).

| Exit | When |
|:----:|:-----|
| `0` | the search endpoint answered (including an empty result) |
| `1` | the search endpoint answered 404 (`cannot enumerate`) |
| `2` | misuse: a non-http URL |
| `3` | transport or auth failure |

Recipes:

```bash
# Show what versions a consumer could resolve.
nxr ls "$BASE" | tail -5

# Feed a human review, not a pipeline: ls is best-effort by design.
nxr ls "$BASE/1.4.0/" --assets
```

`--json` prints one object per line: `{"version":"1.4.0"}` or `{"name":"app.zip"}`.

## Layout helpers

### nxr channel get

`nxr channel get <URL>`

Print the token of a channel ref, a file at any name whose content is one version token.

```console
$ nxr channel get https://nexus.example.com/repository/raw-main/stable
unset
$ echo $?
0
$ nxr channel get https://nexus.example.com/repository/raw-main/stable
1.4.0
$ echo $?
0
```

The second call ran after `channel set` wrote `1.4.0`, and the sequence continues in the next section.
An unset channel is exit 0 with the word `unset`: absence is an answer, not a failure.

| Exit | When |
|:----:|:-----|
| `0` | token printed, or `unset` on 404 |
| `2` | misuse: a non-http URL |
| `3` | transport failure other than 404 |

Recipes:

```bash
# What would a consumer resolve to right now?
nxr channel get "$BASE/stable"

# Fail a deploy when the channel is not set.
test "$(nxr channel get "$BASE/stable")" != unset
```

`--json`: `{"token":"1.10.1","url":"…"}`, with `token` `null` when unset and exit 0 either way.

### nxr channel set

`nxr channel set <URL> <TOKEN> [--if-forward]`

Write the token.
A channel is any name, so `latest`, `nightly`, `stable` and `prod` all work, and `nxr` has no reserved list.

| Flag | Meaning |
|:-----|:--------|
| `--if-forward` | keep the current token when it already compares ≥ the new one (dotted-numeric order) |

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/stable 1.4.0
channel: set https://nexus.example.com/repository/raw-main/stable → 1.4.0
$ nxr channel set https://nexus.example.com/repository/raw-main/stable 1.3.9 --if-forward
channel: kept https://nexus.example.com/repository/raw-main/stable at 1.4.0 (forward-only)
$ echo $?
0
$ nxr channel set https://nexus.example.com/repository/raw-main/stable 1.10.0 --if-forward
channel: set https://nexus.example.com/repository/raw-main/stable → 1.10.0
$ echo $?
0
```

A kept channel is exit 0: the guard did its job.
The comparison is dotted-numeric, so `1.10.0` is forward of `1.9.9` and a release beats its own `rc1` (the full rules live in [the protocol page](protocol.md#channel-ref)).

| Exit | When |
|:----:|:-----|
| `0` | written, or kept by the guard |
| `2` | misuse: a token that is not one non-empty line |
| `3` | transport or auth failure |

Recipes:

```bash
# Promote after verification, never backwards by accident.
nxr channel set "$BASE/stable" "$VERSION" --if-forward

# Move nightly unconditionally.
nxr channel set "$BASE/nightly" "$(date +%Y.%m.%d)"
```

`--json`: `{"from":"1.10.0","outcome":"written","token":"1.10.1","url":"…"}` or `{"current":"1.10.1","outcome":"skipped","url":"…"}`.

### nxr verify

`nxr verify <DIR> [--manifest FILE|-]`

Check local bytes, markers and digests.
No network: the run is entirely offline.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|->` | check exactly these names instead of the whole directory |

Every checked name must be complete: bytes present, sibling present, digest matching.
Markerless, broken or missing names are reported and the exit is 1.

```console
$ nxr verify dist/1.4.0/
verify: 2 ok, FAILED: none
uploaded 0, downloaded 0, skipped 2
$ echo $?
0
$ printf 'tampered' >> dist/1.4.0/pinned.xml
$ nxr verify dist/1.4.0/
error: incomplete: pinned.xml
hint: rerun the same command; finished names are skipped and the rest is retried
$ echo $?
1
```

| Exit | When |
|:----:|:-----|
| `0` | every checked name is complete |
| `1` | at least one name is `Incomplete`, and the error line names them |
| `2` | misuse: the directory does not exist, or the manifest is unreadable |

Recipes:

```bash
# Post-download integrity gate in a deploy job.
nxr verify deploy/

# Verify only what the manifest names, before signing.
nxr verify dist/1.4.0/ --manifest dist/1.4.0/manifest.json
```

### nxr doctor

`nxr doctor [URL]`

Diagnose the setup before a pipeline runs.
Local checks always execute: credentials (resolved presence and source, never values), TLS mode, worker and timeout settings.
With a URL: one `HEAD` probe for reachability, where any HTTP answer counts as reachable.

```console
$ env -u NXR_USERNAME -u NXR_PASSWORD -u NXR_AUTH nxr doctor
doctor:
  [ FAIL ] credentials: none found: anonymous requests; pass -u or export NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
error: misuse: 1 check(s) failed
hint: check the command line arguments
$ echo $?
2
```

A probe that cannot connect is a transport failure:

```console
$ nxr doctor https://nexus.example.com/repository/raw-main/
doctor:
  [  ok  ] credentials: resolved from NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [ FAIL ] probe: transport: …: error sending request for url (…)
error: transport: …: 1 check(s) failed
hint: check the network; transfers are resumable, rerunning is safe
$ echo $?
3
```

`doctor` always renders the human report, `--json` or not.

| Exit | When |
|:----:|:-----|
| `0` | all checks passed |
| `2` | a local gap, for example no credentials anywhere |
| `3` | the probe could not reach the server |

Recipes:

```bash
# First line of every CI pipeline: fail fast with a readable reason.
nxr doctor "$BASE" || exit $?

# Check TLS-breaking-proxy setups without sending credentials over argv.
NXR_USERNAME=ci-bot NXR_PASSWORD="$PASS" nxr doctor "$BASE"
```

## Output

Human output goes to stdout, errors and `hint:` lines to stderr.
`--json` switches stdout to machine form:

- primitives (`get` with `-o`, `put`, `head`, `sha`) and `channel` print **one JSON object**.
- transfers (`up`, `down`) and `verify` print an **NDJSON event stream** (`plan`, `artifact`, `retrying`, `summary`).
- the body of `get` without `-o` is raw bytes on stdout, so do not mix it with `--json`.

The NDJSON shapes are fixed by golden tests in the core crate, and they are the same `Event::to_json()` output the Rust API emits, documented variant by variant in [the API page](api.md#events).
A captured stream:

```json
{"download":["app-1.4.0.zip"],"event":"plan","skip":[],"upload":[]}
{"done":0,"event":"artifact","name":"app-1.4.0.zip","state":"downloading","total":38}
{"done":38,"event":"artifact","name":"app-1.4.0.zip","state":"downloading","total":38}
{"done":38,"event":"artifact","name":"app-1.4.0.zip","state":"done","total":38}
{"downloaded":1,"event":"summary","failed":[],"skipped":0,"uploaded":0}
```

`-q` trims to the final summary line, `-v` adds plan names and retry details.
With `-v`, every plan name is echoed once when the plan is printed and again when its transfer settles:

```console
$ nxr -v up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 0 to upload, 0 to download, 4 up to date
up: 0 sent, 0 fetched, 4 skipped
○ app-1.4.0.zip
○ bom/linux-x86_64.json
○ pinned.xml
○ release-notes.txt
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ pinned.xml skipped
○ release-notes.txt skipped
uploaded 0, downloaded 0, skipped 4
```

## Exit codes

Four codes cover every failure, and the mapping from error to code has one home, [`Error::exit_code`](errors.md#the-taxonomy).

| Code | Class | Representative causes | Commands that hit it |
|:----:|:------|:----------------------|:---------------------|
| `0` | ok | transfer converged, digest verified, status reported, channel written or kept | all |
| `1` | data | `mismatch`, `incomplete`, `missing`, `cannot enumerate` | `up`, `down`, `verify`, `ls` |
| `2` | misuse | bad flags, unsafe names, half-set credentials, missing local paths, non-http URLs | all |
| `3` | transport | auth failures, connection resets, stalls, 5xx after retries, 404 and other unexpected statuses | all except `verify` |

`verify` is offline and cannot produce `3`.
`head` reports statuses as results and exits `0` on any answer. Only a dead connection gives `3`.
Every error also prints a `hint:` line on stderr: the full [taxonomy with hints](errors.md) and the [incident playbook](../how-to/troubleshoot.md) cover what to do next.

Where to go from here:

- publish and consume workflows: [how-to guides](../how-to/publish.md).
- what the CLI puts on the wire: [the protocol](protocol.md).
- embedding the same operations in Rust: [the API](api.md).
