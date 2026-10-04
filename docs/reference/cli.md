# CLI reference

`nxr` moves files to and from a Nexus raw repository.
Single-object commands: `get`, `put`, `head`, `sha`.
Directory transfers with sha-sibling verification: `up`, `down`, `mirror`.
Deletion of an enumerated version or a pointer file: `rm`, `point --clear`.
Layout helpers: `channel get`, `channel set`, `verify`, `doctor`, `ls`.

Every invocation is self-sufficient: the URL is a command-line argument and credentials come from `-u` or the environment.
There is no config file, no profile and no state directory.

The transcripts on this page are real output of `nxr`.
The mock scenarios ran against `crates/mock-nexus`, the `ls` capture against a live Nexus Repository, and hosts are shown as `nexus.example.com` either way.
`just demo` runs a full session against the same mock.
Exit codes are quoted as observed.

## Credentials and URLs

Three credential sources, in precedence order:

| Source | Form | Typical use |
|:-------|:-----|:------------|
| `-u user:pass` | plain `user:pass`, curl style | interactive one-offs; the value is visible in `ps` and shell history |
| `NXR_AUTH` | base64 of `user:pass` | CI, where argv must stay clean |
| `NXR_USERNAME` + `NXR_PASSWORD` | the readable form | CI with masked variables |

`-u` wins over the environment.
`NXR_USERNAME` and `NXR_PASSWORD` must be set together: exactly one alone is a usage error (exit 2), never a silent anonymous call.
Credential values never appear in output, logs or `--json` events.

```bash
printf 'ci-bot:%s' "$TOKEN" | base64        # produce the NXR_AUTH value
export NXR_AUTH="Y2ktYm90OnRva2Vu"
nxr ls https://nexus.example.com/repository/raw-main/
```

`doctor` reports which source resolved without printing the value:

```console
$ env NXR_AUTH=Y2ktYm90OnRva2Vu nxr doctor https://nexus.example.com/repository/raw-main/1.4.0/
doctor:
  [  ok  ] credentials: resolved from NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] probe: HEAD https://nexus.example.com/repository/raw-main/1.4.0/ → HTTP 404
  all checks passed
$ echo $?
0
```

The base URL is normalized: `https://host/raw` and `https://host/raw/` address the same directory.
Only `http`/`https` with a host are accepted, and credentials in the URL's userinfo are rejected.

Proxy environment variables work as in curl: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY` are read from the environment and honored on every request.
Proxy values never appear in logs or output.

## Global flags

All flags are global: they may appear before or after the subcommand.

| Flag | Default | Meaning |
|:-----|:--------|:--------|
| `-u, --user <USER:PASS>` | env | credentials for this call, takes precedence over `NXR_AUTH` and `NXR_USERNAME`/`NXR_PASSWORD` |
| `--workers <N>` | `8` | parallel artifact transfers, accepted range `1..=64` |
| `--retry <N>` | `4` | attempts per HTTP request |
| `--connect-timeout-secs <N>` | `15` | TCP connect timeout in seconds |
| `--stall-secs <N>` | `30` | abort a transfer when no bytes arrive for this many seconds |
| `--tls-insecure` | off | skip certificate verification |
| `--json` | off | machine output, see [output](#output) |
| `-q, --quiet` | off | only the final summary line |
| `-v, --verbose` | off | plan names and transfer starts |

## nxr get

```
nxr get <URL> [-o FILE] [--continue]
```

GET a URL to a file or stdout.
Without `-o` the body goes to stdout in a single attempt: a retry after the body started would duplicate bytes.
With `-o` the bytes stream into `<FILE>.part`, which is renamed to `FILE` only after the transfer completes.

| Flag | Meaning |
|:-----|:--------|
| `-o, --out <FILE>` | output file; stdout when omitted |
| `--continue` | resume from an existing `<FILE>.part` through a `Range: bytes=N-` request |

```console
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
sample app payload for documentation capture
$ echo $?
0
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip
get: 45 bytes → app.zip
$ echo $?
0
```

Resuming from a 20-byte part appends through a Range request and renames when complete:

```console
$ head -c 20 dist/1.4.0/app-1.4.0.zip > app.zip.part
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip --continue
get: 45 bytes → app.zip (resumed from 20)
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | body delivered |
| `1` | local I/O failure around the part file |
| `2` | misuse: a non-http URL |
| `3` | 404, auth failure, transport exhaustion: see [errors](errors.md#captured-transcripts) |

`--json` with `-o` prints the artifact events and then one final object, where `sha256` is the digest of the written file and `resumed_from` the part offset:

```json
{"done":0,"event":"artifact","name":"app.zip","state":"downloading","total":45}
{"done":45,"event":"artifact","name":"app.zip","state":"downloading","total":45}
{"bytes":45,"ok":true,"out":"app.zip","resumed_from":0,"sha256":"a8a24209…","url":"…"}
```

Without `-o` the stdout body is raw bytes and nothing else: stdout mode emits no progress events.
`--json` without `-o` is refused as misuse (exit 2), and the hint names the two ways out: pass `-o FILE` or drop `--json`.

## nxr put

```
nxr put <URL> -f FILE [--sha]
```

PUT a file as the URL's bytes, streamed, with `Content-Length`.
PUTs are idempotent: a broken attempt is replayed whole.

| Flag | Meaning |
|:-----|:--------|
| `-f, --file <FILE>` | the file to send |
| `--sha` | also PUT `<URL>.sha256` with a sha256sum-style marker, digest computed on the fly |

```console
$ nxr put https://nexus.example.com/repository/raw-main/1.4.1/app-1.4.1.zip -f app-1.4.1.zip --sha
put: 45 bytes + marker a8a242091894256798c34ed08c990bccb4a275883335d9209f9954b12e36ac1f → https://nexus.example.com/repository/raw-main/1.4.1/app-1.4.1.zip
$ echo $?
0
$ nxr put https://nexus.example.com/repository/raw-main/1.4.0/r16.bin -f r16.bin
put: 16 bytes → https://nexus.example.com/repository/raw-main/1.4.0/r16.bin (no marker)
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | bytes (and marker, when asked) stored |
| `1` | local I/O: the input file is missing or unreadable |
| `2` | misuse: a non-http URL |
| `3` | auth failure, 5xx after retries, unexpected status |

`--json` prints the artifact and retrying events, then one final object, where `marker` is `null` without `--sha`:

```json
{"bytes":45,"marker":null,"ok":true,"url":"…"}
```

## nxr head

```
nxr head <URL>
```

Status, byte size and content type, taken from the response headers.
The three fields print as `-` when the server sends no size or content type.
`head` never fails on a status: the answer is the result, which makes it the safe preflight in scripts.

```console
$ nxr head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
head: 200 45 -
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

`--json` prints one object, where `size` and `content_type` are `null` when the server sends none:

```json
{"content_type":null,"size":45,"status":200,"url":"…"}
```

## nxr sha

```
nxr sha <FILE|URL>
```

The sha256 of a local file or a remote object, streamed.
A local file never touches the network.

```console
$ nxr sha dist/1.4.0/app-1.4.0.zip
a8a242091894256798c34ed08c990bccb4a275883335d9209f9954b12e36ac1f
$ nxr sha https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
a8a242091894256798c34ed08c990bccb4a275883335d9209f9954b12e36ac1f
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | digest printed |
| `2` | misuse: a missing local file, a non-http URL |
| `3` | transport or auth failure on a URL source |

`--json`: `{"sha256":"a8a24209…","source":"dist/1.4.0/app-1.4.0.zip"}`.

## nxr up

```
nxr up <SRC_DIR> <DST_URL> [--manifest FILE|URL|-] [--no-sha] [--dry-run] [--claim-first <NAME>]
```

Upload a local directory.
The directory is scanned, every file becomes an artifact whose relative path is its name, and the [symmetric diff](protocol.md#the-symmetric-diff) against the server decides what moves.
Hidden entries (leading `.`) and `*.sha256` siblings are not artifacts.
Repeated names transfer again, identical ones are skipped, and an interrupted `up` is finished by repeating the same command.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | restrict the transfer to these names, all of which must exist locally |
| `--no-sha` | skip marker generation and marker uploads, bytes only |
| `--dry-run` | print the plan without transferring anything |
| `--claim-first <NAME>` | upload this one name first and alone, before any other name starts |

Markers are on by default, in both directions:

- existing local `<name>.sha256` siblings are verified against the bytes and uploaded.
- missing local siblings are generated from the bytes.
- a local sibling that does not match its bytes refuses the run (`mismatch`, exit 1) before any byte moves.

First run of a three-file directory:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 3 to upload, 0 to download, 0 up to date
↑ bom/linux-x86_64.json ok
↑ app-1.4.0.zip ok
↑ pinned.xml ok
uploaded 3, downloaded 0, skipped 0
$ echo $?
0
```

Re-running converges: the plan empties and every name reports `skipped`.

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 0 to upload, 0 to download, 3 up to date
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ pinned.xml skipped
uploaded 0, downloaded 0, skipped 3
$ echo $?
0
```

A new file uploads alone, and `--dry-run` shows the plan of exactly that:

```console
$ printf 'release notes\n' > dist/1.4.0/release-notes.txt
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --dry-run
skip app-1.4.0.zip
skip bom/linux-x86_64.json
skip pinned.xml
upload release-notes.txt
$ echo $?
0
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 1 to upload, 0 to download, 3 up to date
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ pinned.xml skipped
↑ release-notes.txt ok
uploaded 1, downloaded 0, skipped 3
$ echo $?
0
```

With `--json`, `--dry-run` prints one object per plan line:

```json
{"action":"skip","name":"app-1.4.0.zip"}
{"action":"upload","name":"manifest.json","size":2}
```

`--claim-first <NAME>` uploads the named file first and alone.
A failed claim aborts the run with nothing else sent.
A claim name outside the scanned directory is misuse:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/claim/ --claim-first nope.txt
error: misuse: claim-first: nope.txt is not among the scanned names of dist/1.4.0/
hint: check the command line arguments
$ echo $?
2
```

| Exit | When |
|:----:|:-----|
| `0` | plan executed, remote converged |
| `1` | data refusal: `mismatch`, `missing` (a `--manifest` name absent locally), `incomplete`; local I/O failures |
| `2` | misuse: not a directory, empty directory, unsafe name, a `--claim-first` name outside the directory |
| `3` | transport or auth failure: the `failed:` list names what did not land |

## nxr down

```
nxr down <SRC_URL> <DST_DIR> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--fresh] [--dry-run]
```

Download a remote directory into a local one, the mirror of `up`.
Every artifact is streamed into a part file, hashed on the fly, checked against the remote marker when one exists, then renamed into place and given a local sibling computed from the received bytes.
`--dry-run` probes the server and prints the plan (`upload`/`download`/`skip` lines, exactly like `up --dry-run`) without writing into `DST_DIR`.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | enumeration source: a local file, a URL, or `-` for stdin |
| `--name <NAME>` | one explicit name, repeat as needed |
| `--ls` | best-effort enumeration through the server search API |
| `--fresh` | ignore existing part files, every name downloads from zero |

Resuming is the default: a rerun picks up the part files of a killed run through `Range` requests.
`down` must know what to fetch, and Nexus raw has no guaranteed directory listing.
Without flags it reads the convention first: a `manifest.json` at the directory URL.
If that is absent the run refuses with `cannot enumerate` (exit 1) and a hint naming the three explicit sources (see [the refusal transcript](errors.md#enumeration-refusal-exit-1)).

```console
$ nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ \
    --name app-1.4.0.zip --name pinned.xml
plan: 0 to upload, 2 to download, 0 up to date
↓ app-1.4.0.zip ok
↓ pinned.xml ok
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
uploaded 0, downloaded 2, skipped 0
$ echo $?
0
```

The downloaded directory verifies offline, because every fetched name gets a local sibling:

```console
$ cat vendor/app/app-1.4.0.zip.sha256
a8a242091894256798c34ed08c990bccb4a275883335d9209f9954b12e36ac1f  app-1.4.0.zip
$ nxr verify vendor/app/
verify: 2 ok, FAILED: none
uploaded 0, downloaded 0, skipped 2
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | plan executed, local copies complete |
| `1` | data refusal: `cannot enumerate`, `missing`, `mismatch` with the remote sibling; local I/O failures |
| `2` | misuse: an unsafe `--name` |
| `3` | transport or auth failure |

Part files are named `.nxr-part-<32 hex>` and are stable per artifact name, so a rerun finds its resume fuel without a state file.
`--fresh` ignores the parts and downloads every name from zero.
A resumed part that belongs to an older remote version fails the digest check and is discarded once: the name restarts from zero under the same digest check.
A fresh download that still diverges refuses the run (exit 1), so nothing divergent is ever written.

## nxr mirror

```
nxr mirror <SRC_URL> <DST_URL> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--dry-run]
```

Pour a version from one repository into another: the source is only read, the destination receives bytes and markers through `up`'s write order.
The enumeration lives at the source, with the same sources as `down`: a `manifest.json` at the directory URL, `--manifest`, repeatable `--name`, or best-effort `--ls`.
Without any source the run refuses with `cannot enumerate` (exit 1).
`--dry-run` probes both sides and prints the plan (`copy <name>` for names that would transfer, `skip <name>` for converged ones) without writing to the destination.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | enumeration source: a local file, a URL, or `-` for stdin |
| `--name <NAME>` | one explicit name, repeat as needed |
| `--ls` | best-effort enumeration through the server search API |

Every name travels through a staging directory: a Range-aware GET from the source, a digest check against the source marker, then a PUT of the bytes and of the `<name>.sha256` marker on the destination.
The destination diff follows `up`'s rules: an identical complete copy is skipped, a diverging complete copy refuses the run, an unfinished copy is completed.
When the enumeration leads with `version.json`, the conventional version document, that name is claimed: it transfers alone before any other name starts, and a failed claim aborts the run with nothing else sent.

`--json` prints the transfer event stream, a `downloading` and an `uploading` sequence for every moved name:

```json
{"download":[],"event":"plan","skip":[],"upload":["a.zip"]}
{"done":0,"event":"artifact","name":"a.zip","state":"downloading","total":35}
{"done":35,"event":"artifact","name":"a.zip","state":"downloading","total":35}
{"done":0,"event":"artifact","name":"a.zip","state":"uploading","total":35}
{"done":35,"event":"artifact","name":"a.zip","state":"uploading","total":35}
{"done":35,"event":"artifact","name":"a.zip","state":"done","total":35}
{"downloaded":0,"event":"summary","failed":[],"removed":0,"skipped":0,"uploaded":1}
```

| Exit | When |
|:----:|:-----|
| `0` | plan executed, the destination holds every enumerated name |
| `1` | data refusal: `cannot enumerate`, a diverging complete copy at the destination (`mismatch`); local I/O around the staging directory |
| `2` | misuse: an unsafe `--name` |
| `3` | transport or auth failure, on either side |

## nxr rm

```
nxr rm <SRC_URL> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--dry-run]
```

Delete a version, name by name: the `.sha256` marker of every enumerated name goes first, then the bytes, so nobody ever sees a complete object mid-delete ([the protocol](protocol.md#deletion)).
The enumeration source is mandatory and shared with `down`: a `manifest.json` at the directory URL, `--manifest`, repeatable `--name`, or best-effort `--ls`.
Without any source the run refuses with `cannot enumerate` (exit 1).

Deletion is by name, not by content.
Divergence is never checked: a diverging or broken remote copy is deleted like any other, because deletion is irreversible by definition.
404 is a normal answer, so `rm` is idempotent: the rerun re-enumerates, collects only 404s and still exits 0.
The `manifest.json` that named the list survives it, because it is the source that keeps the rerun working.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|URL\|->` | enumeration source: a local file, a URL, or `-` for stdin |
| `--name <NAME>` | one explicit name, repeat as needed |
| `--ls` | best-effort enumeration through the server search API |
| `--dry-run` | probe and print the plan: `rm <name>` for present names, `missing <name>` for absent ones, nothing deleted |

A full version goes away in one call:

```console
$ nxr rm --dry-run https://nexus.example.com/repository/raw-main/1.14.0/
rm a.zip
rm b.bin
rm version.json
$ echo $?
0
$ nxr rm https://nexus.example.com/repository/raw-main/1.14.0/
× a.zip removed
× b.bin removed
× version.json removed
removed 3, skipped 0
$ echo $?
0
```

The rerun is a normal 404 walk:

```console
$ nxr rm https://nexus.example.com/repository/raw-main/1.14.0/
○ a.zip missing
○ b.bin missing
○ version.json missing
uploaded 0, downloaded 0, skipped 3
$ echo $?
0
```

A read-only repository refuses before anything moves:

```console
$ nxr rm https://nexus.example.com/repository/raw-main/1.14.0/
uploaded 0, downloaded 0, skipped 0
failed: a.zip
error: read-only: https://nexus.example.com/repository/raw-main/1.14.0/a.zip.sha256: HTTP 403
hint: the repository answered 403 to DELETE: it is read-only or the credentials lack write access; rerunning is safe, nothing was removed
$ echo $?
1
```

| Exit | When |
|:----:|:-----|
| `0` | every name deleted or already absent; also the `--dry-run` plan |
| `1` | data refusal: `cannot enumerate`, the read-only repository (403/405); local I/O failures |
| `2` | misuse: an unsafe `--name` |
| `3` | transport or auth failure: the `failed:` list names where the run stopped |

`--json` prints one event per name and the summary last.
A rerun emits `missing` events and still exits 0:

```json
{"event":"removing","name":"a.zip"}
{"event":"removed","name":"a.zip"}
{"event":"removing","name":"b.bin"}
{"event":"missing","name":"b.bin"}
{"downloaded":0,"event":"summary","failed":[],"removed":1,"skipped":1,"uploaded":0}
```

`--dry-run --json` prints one object per plan line:

```json
{"action":"rm","name":"a.zip","size":23}
{"action":"missing","name":"ghost.bin"}
```

## nxr point --clear

```
nxr point --clear <URL>
```

DELETE the pointer file the URL names: the channel ref, retired.
There is no `--if-forward` here and no comparison at all: clearing is an action, not a move in version order.
An already-absent pointer is a normal outcome (`absent`, exit 0), so the command is idempotent.
`point` without `--clear` is misuse (exit 2): deleting is the only operation the command has.

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/stable 1.4.0
channel: set https://nexus.example.com/repository/raw-main/stable → 1.4.0
$ nxr point --clear https://nexus.example.com/repository/raw-main/stable
point: cleared https://nexus.example.com/repository/raw-main/stable
$ echo $?
0
$ nxr point --clear https://nexus.example.com/repository/raw-main/stable
point: absent https://nexus.example.com/repository/raw-main/stable
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | the pointer is deleted, or was already absent |
| `1` | the read-only repository refused (403/405) |
| `2` | misuse: `point` without `--clear`, a non-http URL |
| `3` | transport or auth failure |

`--json` prints one object per outcome:

```json
{"outcome":"cleared","url":"https://nexus.example.com/repository/raw-main/stable"}
```

```json
{"outcome":"absent","url":"https://nexus.example.com/repository/raw-main/stable"}
```

## nxr ls (experimental)

```
nxr ls <URL> [--assets]
```

List versions under a repository or group URL, or objects under a directory URL.
Both go through the Nexus search API, which is not guaranteed to exist or to behave uniformly across server releases.

| Flag | Meaning |
|:-----|:--------|
| `--assets` | list the object names under a directory URL instead of versions |

At a repository root the command lists the first path segment of every indexed asset, which on a well-formed store is the version directory.
Loose files at the root appear as names too.
This capture was taken against a live Nexus Repository with a neutralized host:

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

Measured caveat from the same live server: for a group URL the search request filters by `group`, and on that Nexus release the parameter matched Maven coordinates rather than raw path prefixes, so a group-scoped `ls` returned an empty list with exit 0.
Treat `ls` as a convenience only.
`down` takes its names from manifests instead of from listings, and the enumeration guarantees live in [the protocol page](protocol.md#enumeration).

| Exit | When |
|:----:|:-----|
| `0` | the search endpoint answered (including an empty result) |
| `1` | the search endpoint answered 404 (`cannot enumerate`) |
| `2` | misuse: a non-http URL, or a URL outside `/repository/<name>/<group…>/` |
| `3` | transport or auth failure |

`--json` prints one object per line: `{"version":"1.4.0"}` or `{"name":"app.zip"}`.

## nxr channel get

```
nxr channel get <URL>
```

Print the token of a channel ref: a file at any name whose content is one version token.
An unset channel is exit 0 with the word `unset`: absence is an answer, not a failure.

```console
$ nxr channel get https://nexus.example.com/repository/raw-main/stable
unset
$ echo $?
0
```

| Exit | When |
|:----:|:-----|
| `0` | token printed, or `unset` on 404 |
| `2` | misuse: a non-http URL |
| `3` | transport failure other than 404 |

`--json` prints one object, with `token` `null` when unset and exit 0 either way:

```json
{"token":null,"url":"…"}
```

## nxr channel set

```
nxr channel set <URL> <TOKEN> [--if-forward]
```

Write the token.
A channel is any name: `latest`, `nightly`, `stable` and `prod` all work, and `nxr` has no reserved list.

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

`--json` prints one object per outcome:

```json
{"from":null,"outcome":"written","token":"1.4.0","url":"…"}
```

```json
{"current":"1.4.0","outcome":"skipped","url":"…"}
```

## nxr verify

```
nxr verify <DIR> [--manifest FILE|-]
```

Check local bytes, markers and digests.
No network.
Every checked name must be complete: bytes present, sibling present, digest matching.
Markerless, broken or missing names are reported and the exit is 1.

| Flag | Meaning |
|:-----|:--------|
| `--manifest <FILE\|->` | check exactly these names instead of the whole directory |

```console
$ nxr verify dist/1.4.0/
verify: 3 ok, FAILED: none
uploaded 0, downloaded 0, skipped 3
$ echo $?
0
$ printf 'tampered' >> dist/1.4.0/pinned.xml
$ nxr verify dist/1.4.0/ --manifest dist/1.4.0/manifest.json
uploaded 0, downloaded 0, skipped 2
failed: pinned.xml
error: incomplete: pinned.xml
hint: rerun the same command; finished names are skipped and the rest is retried
$ echo $?
1
```

| Exit | When |
|:----:|:-----|
| `0` | every checked name is complete |
| `1` | at least one name is incomplete, or a local I/O failure such as a missing directory |
| `2` | misuse: the manifest is not JSON |

## nxr doctor

```
nxr doctor [URL]
```

Diagnose the setup before a pipeline runs.
Local checks always execute: credentials (resolved source, never values), TLS mode, worker and timeout settings.
Settings are validated, not just echoed: workers outside `1..=64`, `--retry 0` or a zero timeout fail the check.

With a URL: one `HEAD` probe for reachability.
Any status below 500 passes, except 401 and 403: the server rejected the credentials, and the probe fails with the detail `credentials rejected`.
A 5xx or a connection failure fails the probe as transport.

Missing credentials are a warning, not a failure: the run reports `none found` and still exits 0, because anonymous requests are a legitimate setup against a read-open repository.

A passing run, probe included, is shown in [credentials and URLs](#credentials-and-urls).
A probe that cannot connect is a transport failure:

```console
$ env NXR_AUTH=Y2ktYm90OnRva2Vu nxr doctor https://nexus.example.com/repository/raw-main/
doctor:
  [  ok  ] credentials: resolved from NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [ FAIL ] probe: transport: https://nexus.example.com/repository/raw-main/: error sending request for url (https://nexus.example.com/repository/raw-main/)
error: transport: https://nexus.example.com/repository/raw-main/: 1 check(s) failed
hint: check the network; transfers are resumable, rerunning is safe
$ echo $?
3
```

The report prints one line per check.
`--json` replaces the human report with one object per check on stdout:

```json
{"check":"credentials","detail":"resolved from NXR_AUTH","ok":true}
```

| Exit | When |
|:----:|:-----|
| `0` | all checks passed, warnings included |
| `2` | a local check failed: TLS verification off, settings out of range |
| `3` | the probe failed: unreachable, a 5xx, or the server rejected the credentials (401/403) |

## Output

Human output goes to stdout.
`error:` and `hint:` lines go to stderr.
The line vocabulary:

- `plan: N to upload, N to download, N up to date`, once per transfer.
- `↑ name ok`, `↓ name ok`, `○ name skipped` when a name settles.
- `× name removed`, `○ name missing` when `rm` settles a name (`× name` in `-v` before the DELETEs go out).
- `↻ name: retry N (reason)` when an attempt is replayed.
- `uploaded N, downloaded N, skipped N`, the final summary of every transfer.
- `removed N, skipped K`, the final summary of `rm`, replacing the upload/download line when something was deleted.
- `failed: name, name` after the summary, when names did not land.

`-q` trims to the final summary line.
`-v` adds plan names and transfer starts (`→ name (total)`):

```console
$ nxr -v up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 0 to upload, 0 to download, 4 up to date
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

Artifact lines arrive in worker completion order, not plan order.

`--json` switches stdout to NDJSON:

- `up`, `down`, `mirror`, `rm`, `verify`, `get -o` and `put` print the event stream (`plan`, `artifact`, `removing`/`removed`/`missing`, `retrying`, `summary`).
- `head`, `sha`, `channel`, `point` and `doctor` print their objects described in their sections above.
- the body of `get` without `-o` is the only thing on stdout: no events are printed in stdout mode, and `--json` without `-o` is refused as misuse (exit 2).
- a failed run ends stdout with one `error` object before the non-zero exit, carrying the exit `code` and the `hint` that stderr carries:

```json
{"event":"error","code":3,"hint":"check the network; transfers are resumable, rerunning is safe"}
```

A captured transfer stream:

```json
{"download":[],"event":"plan","skip":["app-1.4.0.zip","bom/linux-x86_64.json","pinned.xml"],"upload":["extra.txt","manifest.json"]}
{"done":0,"event":"artifact","name":"app-1.4.0.zip","state":"skipped","total":null}
{"done":0,"event":"artifact","name":"bom/linux-x86_64.json","state":"skipped","total":null}
{"done":0,"event":"artifact","name":"pinned.xml","state":"skipped","total":null}
{"done":0,"event":"artifact","name":"extra.txt","state":"uploading","total":13}
{"done":0,"event":"artifact","name":"manifest.json","state":"uploading","total":2}
{"done":13,"event":"artifact","name":"extra.txt","state":"uploading","total":13}
{"done":2,"event":"artifact","name":"manifest.json","state":"uploading","total":2}
{"done":2,"event":"artifact","name":"manifest.json","state":"done","total":2}
{"done":13,"event":"artifact","name":"extra.txt","state":"done","total":13}
{"downloaded":0,"event":"summary","failed":[],"skipped":3,"uploaded":2}
```

The shapes are pinned by the golden files in `crates/nexus-raw/tests/golden/` and documented variant by variant in [the API page](api.md#events).

## Exit codes

Four codes cover every failure, and the mapping from error to code has one home, [`Error::exit_code`](errors.md#the-taxonomy).

| Code | Class | Representative causes |
|:----:|:------|:----------------------|
| `0` | ok | transfer converged, digest verified, status reported, channel written or kept, version deleted or already absent |
| `1` | data | `mismatch`, `incomplete`, `missing`, `cannot enumerate`, the read-only repository refusing a delete, local I/O failures |
| `2` | misuse | bad flags, unsafe names, half-set credentials, empty directories, non-http URLs, `point` without `--clear`, `get --json` without `-o` |
| `3` | transport | auth failures, connection resets, stalls, 5xx after retries, 404 and other unexpected statuses |

`verify` is offline and cannot produce `3`.
`head` reports statuses as results and exits `0` on any answer.
Only a dead connection gives `3`.
Every error also prints a `hint:` line on stderr: the full [taxonomy with hints](errors.md) and the [incident playbook](../how-to/troubleshoot.md) cover what to do next.

Where to go from here:

- publish and consume workflows: [how-to guides](../how-to/publish.md).
- what the CLI puts on the wire: [the protocol](protocol.md).
- embedding the same operations in Rust: [the API](api.md).
