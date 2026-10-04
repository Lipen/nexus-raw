# Wire protocol

What `nxr` says to the server: the store layout, the objects, the classification that drives transfers, the write order and the Range contract.
This page documents implemented behavior.
A divergence between this page and the code is a bug in one of them.

## The shape of a store

A base URL points at a directory inside a raw repository.
Everything `nxr` writes fits four object kinds:

```text
<base>/<name>            artifact bytes
<base>/<name>.sha256     sha-sibling: "<hex>  <name>\n" (sha256sum -c format)
<base>/manifest.json     conventional name list, the enumeration source for down
<base>/<channel>         a token file: "<token>\n", any name (latest, stable, …)
```

Four methods exist: `GET`, `HEAD`, `PUT` and `DELETE`.
DELETE is the one deliberate extension, restricted to the objects of the enumerated version ([Deletion](#deletion)).
There is no rename, no server-side computation.
A version is just a directory, and a channel is just a file.

| Object kind | Path | Written by | Read by | Deleted by |
|:------------|:-----|:-----------|:--------|:-----------|
| bytes | `<base>/<name>` | `put`, `up`, `down` | `get`, `sha`, `head`, `down` | `rm` |
| sha-sibling | `<base>/<name>.sha256` | `put --sha`, `up`, `down` | `up`, `down`, `verify` | `rm`, before the bytes |
| manifest | `<base>/manifest.json` | `up` (as an ordinary artifact) | `down`, `up --manifest`, `rm` | enumerated by name, else it survives |
| channel ref | `<base>/<channel>` | `channel set` | `channel get` | `point --clear` |

Example: base `https://nexus.example.com/repository/raw-main/`, artifact `bom/linux-x86_64.json`:

```text
GET https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json
GET https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json.sha256
PUT https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json
PUT https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json.sha256
```

### Group repositories

A group URL is a read-only aggregation over several hosted repositories: a read is dispatched to the members in order and the first member answering `2xx` serves the object.
A member's `404` (or any other non-2xx answer, or a broken connection) falls through to the next member, and when nobody answers `2xx` the group answers a plain `404`.
Writes never reach a member: `PUT` and `DELETE` (like every method but `GET`/`HEAD`) are refused with `405`, an `Allow: GET,HEAD` header and no body, so publication targets hosted members only.
The dispatch mirrors the real Nexus group handler: the method switch at `GroupHandler.java:103-112` and the first-2xx member walk (`getFirst`) at `GroupHandler.java:124-166` of `nexus-public`.
The search API is a separate subsystem in real Nexus (it reads the metadata database, not the group dispatch), so group search is not modeled.

## Names

- A name is a relative path: segments through `/`.
- A segment matches `[A-Za-z0-9._-]+` and is 1 to 255 bytes.
- Forbidden: empty segments, `.`/`..` segments, leading or trailing `/`.
- Reserved: the `.sha256` suffix and nothing else.
- `version.json`, `latest`, `nightly` are ordinary names: upload them, download them, point a channel at them.

What a name means is the publisher's convention, not the protocol's.
The grammar lives in `ArtifactName` and is enforced before any byte moves.

## Objects

### sha-sibling

Exactly one strict `sha256sum -c` line next to the bytes:

```text
<64 lowercase hex chars>␠␠<name>\n
```

The digest covers the artifact bytes, never the marker itself.
A marker as `up` wrote it:

```console
$ cat dist/1.4.0/app-1.4.0.zip.sha256
a8a242091894256798c34ed08c990bccb4a275883335d9209f9954b12e36ac1f  app-1.4.0.zip
```

The parse is strict, because a loose parse would bless foreign markers:

| Rule | Rejected input |
|:-----|:---------------|
| exactly one line | two markers concatenated |
| LF only | CRLF |
| exactly two spaces | one or three spaces |
| digest is 64 lowercase hex | uppercase hex, garbage, short digests |
| name is non-empty and contains no double space | `<hex>  \n` |
| trailing newline required | the same line without `\n` |

An artifact is complete when bytes and sibling both exist and the digest matches.
The sibling is the marker of its bytes and is never an artifact in its own right, so `nxr` never uploads a `.sha256` as a standalone name.

### manifest

```json
{"artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json"]}
```

An ordinary file with a conventional role: the enumeration source for `down` and the optional name filter for `up`.
It carries no digests.
Each artifact's sibling does.

| Rule | Behavior |
|:-----|:---------|
| top level | must be a JSON object |
| `artifacts` | required array of strings, every entry passes the name grammar |
| order | free: names may appear in any order |
| duplicates | dropped, first occurrence wins |
| `schema_version` | tolerated, must be `1` when present |
| `version` | tolerated and ignored |

Manifest and channel reads are capped at 16 MiB.
A larger object is refused as a misuse error instead of being slurped into memory.
A `manifest.json` sitting in a published directory is just a file that `up` uploads like any artifact.
The version-document fields are tolerated so a version document and an enumeration manifest can be the same file.

### channel ref

One line: `<token>\n`, written by `channel set` and read by `channel get`.
The bytes of a live channel file:

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/stable 1.10.0
channel: set https://nexus.example.com/repository/raw-main/stable → 1.10.0
$ curl -s https://nexus.example.com/repository/raw-main/stable | xxd
00000000: 312e 3130 2e30 0a                        1.10.0.
```

A channel is any name: `latest`, `stable` and `prod-1` are all ordinary token files.
The token must be exactly one non-empty line with no CR.
`--if-forward` compares tokens in dotted-numeric order:

| Comparison | Result |
|:-----------|:-------|
| `1.10.0` vs `1.9.9` | forward: segments compare numerically |
| `1.4.0` vs `1.14.0` | backward: `4 < 14` numerically |
| `1.0.0` vs `1.0.0-rc1` | forward: a release beats its own prerelease |

The channel ref is the only mutable state besides version directories, and writes are plain PUTs.

## States

For every name, each side (local directory or remote directory) is in one of:

| State | Bytes | Marker | Condition |
|:------|:------|:-------|:----------|
| `Complete` | yes | yes | digest matches |
| `Markerless` | yes | no | bytes without a marker, either under-uploaded or mid-transfer |
| `Broken` | yes | yes | digest mismatch, or the marker does not parse |
| `Absent` | none | none | no bytes; a stray marker is ignored |

Locally, bytes decide: a sibling without bytes is `Absent`.
Remotely, a name costs two requests: `HEAD` on the bytes and `GET` on the sibling, both overlapped by the worker pool.
A remote `HEAD` that answers 2xx without `Content-Length` classifies as `Broken`, never as `Absent`: the object is present but unverifiable, and calling it absent would skip the digest comparison.

## The symmetric diff

The same classification drives `up` and `down`.
The mode only decides what a single completed copy means: send it (up), fetch it (down), or keep it.

| Local | Remote | `up` does | `down` does |
|:------|:-------|:----------|:------------|
| `Complete` | `Complete`, equal digests | skip | skip |
| `Complete` | `Complete`, different digests | **mismatch: refuse** | **mismatch: refuse** |
| `Complete` | `Absent` | upload | skip: the local copy is the truth |
| `Complete` | `Markerless` | upload: the remote copy was never finished | skip |
| `Markerless` | `Complete` | hash and compare, then skip or refuse | download |
| `Markerless` | `Markerless` | **mismatch: refuse, nothing to verify against** | **mismatch: refuse** |
| `Markerless` | `Absent` | hash and upload | **missing**: no completed copy anywhere |
| `Broken` | anything | **mismatch: never overwrite** | **mismatch: never overwrite** |
| anything | `Broken` | **mismatch: a foreign marker is never overwritten** | **mismatch: refuse** |
| `Absent` | `Complete` | **mismatch: up never deletes** | download |
| `Absent` | `Markerless` | **mismatch: refuse** | download and compute the marker |
| `Absent` | `Absent` | **missing** | **missing** |

Three rules bind the whole matrix:

- `Missing` is collected across all names and fires only when no other refusal exists.
- A refusal never transfers, never overwrites, and exits `1` (see [errors](errors.md#the-refusal-rule)).
- `up` treats "remote has what local lacks" as a refusal, because the only way to act on it would be deletion.
  Deletion exists, but as its own command: [Deletion](#deletion).

`--no-sha` does not change the classification (the diff still reads remote siblings) and only skips marker generation and marker uploads, so the server keeps `Markerless` objects.

## Concurrent writers

The never-overwrite rule is decided per run: the diff reads the remote state, then the run writes.
Between that read and the write there is no server-side guard (raw storage carries no conditional PUT contract this tool can rely on), so two writers racing on the same name can still overwrite each other.

The contract is therefore: **one writer per destination prefix**.
In practice this means one CI job (or one mutex, or disjoint `--prefix` ranges) owns each destination tree.
nxr never claims multi-writer safety on a shared prefix, and no mock scenario promises it.

A server that honors conditional PUTs would close this window at the wire level.
That is a protocol change with a compatibility story, not a client fix, and it is deliberately out of scope until a real deployment asks for it.

## The write order

Upload:

```text
1. generate missing local siblings      (hash the bytes, write canonical markers)
2. PUT <name>                           (bytes, in parallel workers)
3. PUT <name>.sha256                    (strictly after the bytes of the same name)
```

Marker-after-bytes is what makes `Markerless` mean "under-uploaded": a client never trusts a marker whose bytes are absent, and a crash between steps 2 and 3 is recoverable by design.
A local sibling that does not match its bytes stops the run at step 1.
The file is never touched and nothing is uploaded.

Download mirrors the order locally:

```text
1. GET <name> into the part file        (hashed on the fly, Range-resumable)
2. digest check against the sibling     (a diverging digest deletes the part)
3. rename part → <name>
4. write <name>.sha256 from the received bytes
```

Every successful download writes its local sibling, so a directory `down` has touched verifies offline even when the server never had a marker.

### Claim first

`up --claim-first <NAME>` changes only the order: the named file uploads first and alone, before any other name starts.
A failed claim aborts the run with nothing else sent, so the server is never left with new bytes behind an old entry file.
A claim name outside the scanned directory is misuse (exit 2).

### Part files

| Surface | Part file | On failure | On success |
|:--------|:----------|:-----------|:-----------|
| `get -o FILE` | `<FILE>.part` | removed on a clean failure; only a killed run leaves one | renamed to `FILE` |
| `down` | `.nxr-part-<32 hex>` per name | kept, because it is the resume fuel | renamed to the name |
| `put` / `up` | none | PUTs replay whole | none |

The `down` part name is a stable hash of the artifact name, so a repeated run finds its fuel without a state file.
`get` in stdout mode is different again: one body attempt and no retries after the body starts, since a retry would duplicate bytes on the terminal.

## Deletion

`rm` and `point --clear` are the one deliberate extension of the write-only protocol.
DELETE is allowed for the objects of the enumerated version only:

| Object | Deleted by |
|:-------|:-----------|
| `<base>/<name>` bytes | `rm`, one DELETE per enumerated name |
| `<base>/<name>.sha256` marker | `rm`, before the bytes of the same name |
| the version document | `rm`, when the manifest lists it: `version.json` is an ordinary name |
| the pointer file | `point --clear <URL>` |

The rules the protocol fixes:

- The deletion order is the reverse of publishing: the marker goes first, then the bytes.
  Between the two DELETEs the object is incomplete, so nobody ever observes a complete object mid-delete.
- 404 is a normal answer, not an error: deletion is idempotent.
  A rerun of the same `rm` re-enumerates and collects only 404s, still with exit 0.
- 403/405 refuse the run as a read-only repository: exit 1 with a hint.
  The refusal fires on the first DELETE, so a refused run has deleted nothing.
- Divergence is never checked when deleting.
  `rm` removes names, not content: a diverging or broken remote copy is deleted like any other.
  This is a deliberate simplification: the digest comparison belongs to transfers, which refuse instead of overwriting, while deletion is irreversible by definition.
- `rm` removes exactly the enumerated names and their markers.
  Other versions, other names and other pointers are not touched.
  The `manifest.json` that named the list survives it: it is the enumeration source that keeps a rerun idempotent, and deleting it is an explicit `--name manifest.json` decision.
- A transport failure stops the run.
  The names already deleted stay deleted, and a rerun finishes the rest.

## Resume: the Range contract

Range requests are the only recovery mechanism, and they need no server cooperation beyond HTTP.
The client sends `Range: bytes=<part-size>-` only when resuming a non-empty part.

| Server answer | Meaning | Client behavior |
|:--------------|:--------|:----------------|
| `206 Partial Content` | the range was honored | append from the part offset |
| `200 OK` | the server ignored the range | restart from zero, rehash the whole body |
| `416 Range Not Satisfiable` | the part already holds the whole object | **finalize the part**: rename and verify |
| anything else | unexpected | retryable only if 5xx, otherwise the run fails |

The `416` finalize covers the crash between the last downloaded byte and the rename: the part is complete, the server says so, and the run finishes instead of restarting.
The resume request pins `Accept-Encoding: identity`, so its offsets always refer to the stored bytes, whatever encodings a normal download negotiates.
The raw exchange, captured against the mock:

```console
$ nxr put https://nexus.example.com/repository/raw-main/1.4.0/r16.bin -f r16.bin
put: 16 bytes → https://nexus.example.com/repository/raw-main/1.4.0/r16.bin (no marker)
$ curl -s -D - -o /dev/null -H 'Range: bytes=4-' https://nexus.example.com/repository/raw-main/1.4.0/r16.bin | grep -E '^HTTP|Content-Range|Content-Length'
HTTP/1.1 206 Partial Content
Content-Length: 12
Content-Range: bytes 4-15/16
$ curl -s -D - -o /dev/null -H 'Range: bytes=16-' https://nexus.example.com/repository/raw-main/1.4.0/r16.bin | grep -E '^HTTP'
HTTP/1.1 416 Range Not Satisfiable
```

The same edge through the client, first a resume from a 12-byte part and then the finalize of a complete part:

```console
$ head -c 12 dist/1.4.0/app-1.4.0.zip > app.zip.part
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip --continue --json
{"done":0,"event":"artifact","name":"app.zip","state":"downloading","total":45}
{"done":45,"event":"artifact","name":"app.zip","state":"downloading","total":45}
{"bytes":45,"ok":true,"out":"app.zip","resumed_from":12,"sha256":"a8a24209…","url":"…"}
$ cp dist/1.4.0/app-1.4.0.zip app.zip.part
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -o app.zip --continue --json
{"bytes":45,"ok":true,"out":"app.zip","resumed_from":45,"sha256":"a8a24209…","url":"…"}
```

!!! warning "Upstream quirk: Range on an empty object"

    A `Range` GET against a zero-byte object answers **500** on the Nexus 3 generation this client targets.
    This is server-side behavior that `nxr` cannot influence, and it only matters when resuming onto a zero-byte object.
    A fresh download never sends a Range header, and neither does the client here: a `Range` header goes out only when the part already holds bytes, and the complete part of a zero-byte object is empty, so the request is a plain unconditional GET.

`down` resumes each name's part file the same way by default, and `--fresh` ignores the parts to start every name from zero.
A resumed part that belongs to an older remote version fails the digest check: `nxr` discards that part once and restarts the name from zero under the same digest check, so a rerun after a remote update self-heals instead of refusing.
Only a fresh download that still diverges from the sibling refuses the run with exit 1.
Uploads cannot resume: a PUT is byte-exact and replayed whole, which is safe because PUTs are idempotent.
A server-side cut of the first PUT attempt demonstrates the replay: the retry sends the object from byte zero, and the stored content is complete.

```console
$ nxr put https://nexus.example.com/repository/raw-main/pp/app.bin -f dist/1.4.0/app-1.4.0.zip --json
{"done":0,"event":"artifact","name":"app-1.4.0.zip","state":"uploading","total":45}
{"done":45,"event":"artifact","name":"app-1.4.0.zip","state":"uploading","total":45}
{"attempt":2,"event":"retrying","name":"app-1.4.0.zip","reason":"transport: https://nexus.example.com/repository/raw-main/pp/app.bin: error sending request for url (https://nexus.example.com/repository/raw-main/pp/app.bin)"}
{"done":0,"event":"artifact","name":"app-1.4.0.zip","state":"uploading","total":45}
{"done":45,"event":"artifact","name":"app-1.4.0.zip","state":"uploading","total":45}
{"bytes":45,"marker":null,"ok":true,"url":"https://nexus.example.com/repository/raw-main/pp/app.bin"}
```

## Enumeration

`up` never needs to enumerate: it scans the local directory.
`down` and `rm` must learn the name list from an explicit source, because Nexus raw has no guaranteed directory listing.

| Source | Flag | Guarantee |
|:-------|:-----|:----------|
| manifest at the directory URL | *(default)* | exact, when `manifest.json` exists there |
| manifest file, URL or stdin | `--manifest` | exact |
| explicit names | `--name` (repeatable) | exact |
| server search API | `--ls` | best-effort, depends on the server release |

Without any source and without `manifest.json` at the directory URL, `down` and `rm` refuse with `cannot enumerate` and do no guessing or HEAD-probing for likely names.
The search API walks `/service/rest/v1/search/assets` with continuation tokens.
It exists on common Nexus 3 releases but is not guaranteed, and on some releases its filters match Maven coordinates rather than raw paths.
Treat `--ls` output as a hint only, and take the plan of record from a manifest or from explicit names.

## Mirroring

`nxr mirror` pours enumerated names from a source directory into a destination one.
The enumeration is the source's (the same table as above), and the destination diff is `up`'s, with the source in the local column:

| Source | Destination | `mirror` does |
|:-------|:------------|:--------------|
| `Complete` | `Complete`, equal digests | skip |
| `Complete` | `Complete`, different digests | **mismatch: refuse** |
| `Complete` | `Markerless` or `Absent` | copy: PUT bytes, then PUT marker |
| `Markerless` | `Complete` | stage the bytes and compare digests: equal skips, different **refuses** |
| `Markerless` | `Markerless` | **mismatch: refuse, nothing to verify against** |
| `Markerless` | `Absent` | copy the bytes, write the marker computed from them |
| `Broken` at either side | | **mismatch: refuse** |
| `Absent` | `Absent` | **missing** |
| `Absent` | anything present | **mismatch: mirroring never deletes** |

Reads are GETs: every copied name is staged through the `down` machinery (a Range-aware GET into a part file, hashed on the fly, verified against the source marker when one exists) and then pushed through the `up` machinery, so the write order is exactly up's and a refusal never reaches the destination.
The staging parts live in a per-(source, destination) directory under the system temp dir.
A rerun resumes them through `206`, and a clean run removes the directory.
When the enumeration leads with the conventional version document `version.json`, it is claimed: it transfers first and alone, and a failed claim aborts the run with nothing else sent.

## Transport

| Aspect | Behavior |
|:-------|:---------|
| auth | `Basic`, attached to every request when credentials resolve; `-u` beats `NXR_AUTH` beats `NXR_USERNAME`+`NXR_PASSWORD` |
| TLS | verified by default (`--tls-insecure` is the only off-switch) |
| retries | up to 4 attempts per request: connect errors, timeouts, body breaks and 5xx retry; other 4xx never, except 429 |
| 429 | retryable: a `Retry-After` pause in seconds (clamped to 1..=60) replaces the backoff; exhausting attempts is exit 3 with a rate-limit hint |
| backoff | 0.5 s × 2ⁿ per attempt, capped at 60 s, plus hash-based jitter ≤ 250 ms |
| stall | no bytes for `--stall-secs` aborts the attempt as retryable (default 30 s) |
| timeouts | connect timeout only, no total-per-artifact timeout, because a big artifact on a slow link is legitimate |
| parallelism | 8 workers by default (`--workers`), one name per worker at a time |
| idempotency | PUTs are byte-exact repeats, a resumed download replays the same bytes, and DELETE of an absent object is a normal 404 |
| compression | a GET body with `Content-Encoding: zstd` decodes transparently, and the client may send `Accept-Encoding: zstd` |

Auth failures bypass the retry loop: a 401/403 is answered once and reported.
5xx responses retry with backoff, which the output makes visible:

```console
$ nxr put https://nexus.example.com/repository/raw-main/flaky/app.bin -f extra.txt
↻ extra.txt: retry 2 (transport: https://nexus.example.com/repository/raw-main/flaky/app.bin: HTTP 503)
↻ extra.txt: retry 3 (transport: https://nexus.example.com/repository/raw-main/flaky/app.bin: HTTP 503)
put: 13 bytes → https://nexus.example.com/repository/raw-main/flaky/app.bin (no marker)
```

Compression is GET-side only, and `zstd` is the only encoding the client understands: PUT bodies never carry `Content-Encoding`, and a store that never compresses sees no behavior change.
Markers and digests always describe the original, decoded bytes, so the marker check is the same check it always was.
Decoding is streaming: a frame that declares its content size fails once the output passes it, and an undeclared frame is bounded by the digest check and the stall timeout rather than by the decoder.

How the layers expose this protocol to Rust callers: [the Rust API](api.md).
