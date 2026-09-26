# Errors and exit codes

Every failure names its kind, the object and both sides of a disagreement — enough to act without re-running under a debugger.
Every error also prints a `hint:` line on stderr telling you what to check next.

## The taxonomy

| Error | Meaning | Exit |
|:------|:--------|:----:|
| `Mismatch { name, detail }` | digest or marker disagreement on a completed artifact | 1 |
| `Incomplete { names }` | local copies are not Complete (before publish, or at `verify`) | 1 |
| `Missing { names }` | requested names exist neither locally nor remotely | 1 |
| `Enumerate { url, reason }` | `down` has no enumeration source and the server has no `manifest.json` | 1 |
| `UnsafeName { name, reason }` | a name failed the grammar or used the reserved `.sha256` suffix | 2 |
| `Misuse` | bad flags, missing file or directory, half-set credentials | 2 |
| `Auth { url, reason }` | 401/403, or credentials required but absent | 3 |
| `Transport { url, detail }` | network, TLS, timeouts — retries exhausted | 3 |
| `Http { status, url }` | any other unexpected status | 3 |

Exit code `1` is a data verdict, `2` a broken invocation, `3` a broken pipe.
The mapping lives in exactly one place, `Error::exit_code` in `crates/nexus-raw-core/src/error.rs`.

## Message anatomy

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
error: mismatch: app.zip: complete on both sides with different digests: local 9f86…, remote 2c26…
hint: the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object
```

- `error:` — every failure goes to stderr in this shape.
- the kind — `mismatch`, `missing`, `cannot enumerate`, …
- the rest — names, digests, URLs.
- `hint:` — the next thing to check, one line, always present except for plain unexpected HTTP statuses.

## Hints

| Error | Hint |
|:------|:-----|
| `Mismatch` | the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object |
| `Incomplete` | rerun the same command; finished names are skipped and the rest is retried |
| `Missing` | the name is absent on both sides; check spelling and the manifest |
| `Enumerate` | pass `--manifest <file|url|->`, repeat `--name`, or use `--ls` when the server has the search API |
| `UnsafeName` | names must be relative paths of `[A-Za-z0-9._-]` segments; the `.sha256` suffix is reserved |
| `Auth` | pass `-u user:pass` or export `NXR_AUTH` (base64 `user:pass`) |
| `Transport` | check the network; transfers are resumable, rerunning is safe |
| `Http` 404 | check the URL path and that the version or object exists |
| `Http` other | — |
| `Misuse` | check the command line arguments |

## Exit codes

| Code | Class | A script should |
|:-----|:------|:----------------|
| `0` | ok | continue |
| `1` | data | stop — a human decides whose content is right |
| `2` | misuse | fix the invocation |
| `3` | transport | retry the job later — resume is free |

`doctor` maps local gaps to `2` (for example no credentials anywhere) and failed reachability to `3`.

## The refusal rule

`Mismatch`, `Incomplete` and `Missing` are refusals, not failures to try.
`nxr` never overwrites a diverging completed artifact, never shadows a remote-only name with `up`, and never writes bytes that failed their digest check.
Recovery is always a decision — fix or delete a copy, republish under a new name — never a retry.

## In NDJSON

Transfer failures appear in the summary's `failed` list:

```json
{"event":"summary","uploaded":0,"downloaded":0,"skipped":1,"failed":["app.zip"]}
```

The `error:` and `hint:` lines still go to stderr.
The process exit code carries the class.

## In the Rust API

The same taxonomy is `nexus_raw_core::Error`:

```rust
match result {
    Err(e) if e.exit_code() == 3 => warn!("transport, retry later: {e}"),
    Err(e) => error!("data or invocation problem: {e}"),
    Ok(summary) => info!("{summary:?}"),
}
```

`Error::exit_code()` is the single mapping from error to exit code — wrappers call it instead of re-deriving one.
