# Errors and exit codes

Every failure names its kind, the object and both sides of a disagreement.
Every error also prints one `hint:` line on stderr telling you what to check next.
The taxonomy has twelve variants and four exit codes, and the mapping between them lives in exactly one place: `Error::exit_code` in `crates/nexus-raw-core/src/error.rs`.

## The taxonomy

The variant set mirrors the error enum, fields included.
Wrappers can construct and match them, because the fields are public.
The hint column quotes `Error::hint()` verbatim: the same string the CLI prints and JSON wrappers receive in the `hint` field.

| Variant | Meaning | Exit | Hint (verbatim) | Typical cause |
|:--------|:--------|:----:|:----------------|:--------------|
| `Mismatch { name, detail }` | digest or sibling disagreement on a completed artifact | 1 | `the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object` | the same name was published twice with different content, or a sibling belongs to a foreign object |
| `Incomplete { names }` | the local bytes+marker+digest chain is not closed | 1 | `rerun the same command; finished names are skipped and the rest is retried` | a previous run stopped mid-transfer, or a file was edited after its marker was written |
| `Missing { names }` | requested names exist neither locally nor remotely | 1 | `the name is absent on both sides; check spelling and the manifest` | a typo, or a manifest entry for something never published |
| `Enumerate { url, reason }` | `down` or `ls` has no enumeration source | 1 | `pass --manifest <file|url|->, repeat --name, or use --ls when the server has the search API` | no `manifest.json` at the directory URL and no flag naming a source |
| `UnsafeName { name, reason }` | a name failed the grammar | 2 | `names must be relative paths of [A-Za-z0-9._-] segments; the .sha256 suffix is reserved` | spaces, unicode, empty segments or the reserved `.sha256` suffix |
| `Misuse(String)` | bad flags, missing files, half-set credentials | 2 | `check the command line arguments` | invocation mistakes the shell cannot catch |
| `Auth { url, reason }` | 401 or 403, or credentials required but absent | 3 | `pass -u user:pass or export NXR_AUTH (base64 user:pass)` | expired token, wrong password, anonymous write attempt |
| `ReadOnly { url, status }` | the repository refuses a deletion: 403/405 to DELETE | 1 | `the repository answered {status} to DELETE: it is read-only or the credentials lack write access; rerunning is safe, nothing was removed` | a read-only deployment, or credentials without write access |
| `Transport { url, detail }` | network, TLS, timeout or stall after retries | 3 | `check the network; transfers are resumable, rerunning is safe` | server down, connection reset, stalled body |
| `Http { status: 404, url }` | the object or version does not exist | 3 | `check the URL path and that the version or object exists` | a typo in the path, or a version never published |
| `Http { status, url }` | any other unexpected status | 3 | `the server answered {status}; check the URL path and the server health` | a proxy answered 429, or the path hit a non-artifact route; a 429 and a 5xx are retryable, the 429 honoring `Retry-After` |
| `ServiceMissing { url, root }` | the service REST API answered 404: not a Nexus, or a version without the endpoint | 3 | `the service API lives at the server root: try {root}/service/rest/v1/repositories` | `service repos` against a server without the management API |
| `Io { path, detail }` | a local filesystem failure | 1 | `check the local filesystem: permissions, space, symlinks; transfers are resumable, rerunning is safe` | a full disk, a missing directory, an unwritable part path |

Exit `1` is a data verdict, `2` a broken invocation, `3` a broken transport.
The same classes appear in the [CLI exit code table](cli.md#exit-codes) and in `Error::exit_code()` for API wrappers.

## Message anatomy

All failures go to stderr in one shape: an `error:` line, then a `hint:` line.
This refusal was produced by a real run: a local sibling that no longer matches its bytes stops the run before any byte moves.

```console
$ nxr up dist-bad/ https://nexus.example.com/repository/raw-main/1.4.0/ --plan
error: mismatch: pinned.xml: local object is broken and must not be overwritten: digest mismatch: sibling 48f92f1ecbe87b5702a77327c4b1da6bb3f78218eb93edfcdb9c21624d430413, actual f7324dec778c7f17852694ef8c8624e9f1f55289010f77bf7f8b56e0d9088989
hint: the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object
$ echo $?
1
```

- `error:`, the prefix every failure starts with on stderr.
- the kind, one of `mismatch`, `missing`, `cannot enumerate`, `unsafe name`, `transport`, …
- the detail, such as names, digests, URLs or the server's own reason.
- `hint:`, the next thing to check, one line.

## Captured transcripts

Each transcript below was produced by a real `nxr` run against the deterministic failure mock (`crates/mock-nexus`), with hosts neutralized to `nexus.example.com`.
The messages and exit codes are verbatim.

### Auth failure (exit 3)

Wrong credentials against a repository that requires auth.
The `Auth` variant's message already carries the remedy, and there are no retries.

```console
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip -u ci-bot:wrong-pass -o app.zip
error: auth: https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip: HTTP 401 Unauthorized; pass -u user:pass or export NXR_AUTH (base64 user:pass)
hint: pass -u user:pass or export NXR_AUTH (base64 user:pass)
$ echo $?
3
```

Missing credentials fail identically, because anonymous access earns the same 401.

### Not found (exit 3)

A 404 becomes `Http { status: 404 }` with its own hint.
`head` is the exception: it reports any status as a result and exits 0.

```console
$ nxr get https://nexus.example.com/repository/raw-main/1.4.0/no-such-object.bin -o x.bin
error: http 404: https://nexus.example.com/repository/raw-main/1.4.0/no-such-object.bin
hint: check the URL path and that the version or object exists
$ echo $?
3
```

### Enumeration refusal (exit 1)

`down` refuses to guess what to fetch.
Nexus raw has no guaranteed directory listing, so a missing source is a data refusal instead of a network error.

```console
$ nxr down https://nexus.example.com/repository/raw-main/1.4.1/ vendor/v141/
error: cannot enumerate: https://nexus.example.com/repository/raw-main/1.4.1/: no manifest.json on the server and no --manifest/--name/--ls given
hint: pass --manifest <file|url|->, repeat --name, or use --ls when the server has the search API
$ echo $?
1
```

### Misuse (exit 2)

Half-set environment credentials are an invocation error, never a silent anonymous call.

```console
$ env NXR_USERNAME=ci-bot nxr head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
error: misuse: NXR_USERNAME and NXR_PASSWORD must be set together
hint: check the command line arguments
$ echo $?
2
```

The same class covers flag values:

```console
$ nxr --workers 0 head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip
error: misuse: --workers must be in 1..=64, got 0
hint: check the command line arguments
$ echo $?
2
```

### Missing name (exit 1)

A name that exists on neither side is collected and refused before any transfer.

```console
$ nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/v3/ --name no-such-file.bin
error: missing: no-such-file.bin: exist nowhere
hint: the name is absent on both sides; check spelling and the manifest
$ echo $?
1
```

### Unsafe name (exit 2)

The grammar rejects the file before anything is hashed or sent.

```console
$ nxr up dist-broken/ https://nexus.example.com/repository/raw-main/broken/
error: unsafe name: bad name.zip: each segment must match [A-Za-z0-9._-]+, be 1..=255 bytes, and not be '.' or '..'
hint: names must be relative paths of [A-Za-z0-9._-] segments; the .sha256 suffix is reserved
$ echo $?
2
```

### Transport (exit 3)

Two flavors: a connection that resets before answering, and a body that stops mid-flight.

```console
$ nxr get https://nexus.example.com/reset/app.bin --retry 1
error: transport: https://nexus.example.com/reset/app.bin: error sending request for url (https://nexus.example.com/reset/app.bin)
hint: check the network; transfers are resumable, rerunning is safe
$ echo $?
3
$ nxr get https://nexus.example.com/slowdata/big.bin --stall-secs 1 --retry 1 -o app.bin
error: transport: https://nexus.example.com/slowdata/big.bin: stalled: no bytes for 1s
hint: check the network; transfers are resumable, rerunning is safe
$ echo $?
3
```

### Foreign sibling (exit 1)

A remote sibling whose digest cannot belong to the bytes is never overwritten.
The per-name failure is collected in the summary, then the run refuses.

```console
$ nxr down https://nexus.example.com/repository/raw-main/foreign/ out/ --name app.bin
plan: 0 to upload, 1 to download, 0 up to date
uploaded 0, downloaded 0, skipped 0
failed: app.bin: downloaded digest bd4ef5a0775f7705a1758f3ecde220fd045cce6a3b3b816fc684aada794bc24a, remote sibling 0000000000000000000000000000000000000000000000000000000000000000
error: mismatch: app.bin: downloaded digest bd4ef5a0775f7705a1758f3ecde220fd045cce6a3b3b816fc684aada794bc24a, remote sibling 0000000000000000000000000000000000000000000000000000000000000000: remote content diverges from its sha-sibling
hint: the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object
$ echo $?
1
```

### Local I/O (exit 1)

A local filesystem failure is data, not misuse: the command line may be perfect and the disk full.

```console
$ nxr put https://nexus.example.com/repository/raw-main/foreign/app.bin -f extra.txt
error: io: extra.txt: No such file or directory (os error 2)
hint: check the local filesystem: permissions, space, symlinks; transfers are resumable, rerunning is safe
$ echo $?
1
```

### Incomplete at verify (exit 1)

`verify` is offline, so every data problem it finds is `Incomplete`.
The summary names what failed, then the error line repeats it.

```console
$ nxr verify dist/1.4.0/ --manifest dist/1.4.0/manifest.json
uploaded 0, downloaded 0, skipped 2
failed: pinned.xml
error: incomplete: pinned.xml
hint: rerun the same command; finished names are skipped and the rest is retried
$ echo $?
1
```

## Exit-code matrix

The per-command tables live in [the CLI page](cli.md#exit-codes).
In short: `0` continue the pipeline, `1` stop and show stderr to a human, `2` fix the invocation and rerun, `3` retry later, because resume makes reruns cheap.

`doctor` reuses the same classes: a failed local check such as TLS verification off or settings out of range exits `2`, a failed reachability probe exits `3` (a 401 or 403 probe fails as `credentials rejected`, still exit `3`).

## The refusal rule

`Mismatch`, `Incomplete` and `Missing` are refusals, not failed attempts.
`nxr` never overwrites a diverging completed artifact, never shadows a remote-only name with `up`, and never writes bytes that failed their digest check.
Recovery is a decision (fix or delete a copy, or republish under a new name), never a retry.
The classification that produces these verdicts is specified in [the symmetric diff](protocol.md#the-symmetric-diff).

## In NDJSON

Transfer-level failures surface in the summary's `failed` list and in `retrying` events rather than as separate error objects.

```json
{"attempt":2,"event":"retrying","name":"extra.txt","reason":"transport: https://nexus.example.com/repository/raw-main/flaky/app.bin: HTTP 503"}
```

```json
{"downloaded":0,"event":"summary","failed":["app.bin: downloaded digest bd4ef5a0775f7705a1758f3ecde220fd045cce6a3b3b816fc684aada794bc24a, remote sibling 0000000000000000000000000000000000000000000000000000000000000000"],"skipped":0,"uploaded":0}
```

The `error:` and `hint:` lines still go to stderr, and the process exit code still carries the class.

## In the Rust API

The taxonomy is `nexus_raw_core::Error`.
`Error::exit_code()` is the single mapping from variant to class, and `Error::hint()` returns the hint string.
Wrappers call these instead of re-deriving either.

```rust
match nxr.up(dir, None, true, None).await {
    Ok(summary) => info!("published: {summary:?}"),
    Err(e) if e.exit_code() == 3 => warn!("transport, retry later: {e}"),
    Err(e) => error!("data or invocation problem, hint: {:?}", e.hint()),
}
```

The diff refusals are a separate type, `nexus_raw_core::Verdict`, which converts into `Error` with `From`, so a refused plan and a refused transfer look identical to a caller.
See [the Rust API page](api.md#errors) for the full picture.

## Where to go next

- Every flag and exit row per command: [CLI reference](cli.md#exit-codes).
- Live-incident recipes: [when it breaks](../how-to/troubleshoot.md).
- Why the wire looks this way: [the wire protocol](protocol.md).
