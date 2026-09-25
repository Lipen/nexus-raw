# Errors and exit codes

Every failure names its kind, the artifact and both sides of a disagreement — enough to act without re-running under a debugger.

## The taxonomy

| Error | Meaning | Exit |
|:------|:--------|:----:|
| `Mismatch { name, detail }` | digest or marker disagreement on a completed artifact | 1 |
| `Incomplete { names }` | local copies are not Complete before publish / at verify | 1 |
| `ClaimDrift { version, detail }` | the remote claim differs from the local one, or does not parse | 1 |
| `Missing { names }` | claimed artifacts exist nowhere | 1 |
| `UnsafeName { name, reason }` | a name failed the grammar or hit a reserved word | 2 |
| `Misuse` | bad flags, missing file or directory, broken config | 2 |
| `Auth { url, reason }` | 401/403, or credentials required but absent | 3 |
| `Transport { url, detail }` | network, TLS, timeouts — retries exhausted | 3 |
| `Http { status, url }` | any other unexpected status | 3 |

## Message anatomy

```console
$ nxr up --profile main --dir dist/1.4.0
error: mismatch: app.zip: complete on both sides with different digests: local 9f86…, remote 2c26…
```

- `error:` — every failure goes to stderr in this shape (or as NDJSON when `--json`).
- `mismatch:` — the kind.
- the rest — names, digests, URLs, with the [protocol](protocol.md) guarantee that the failed name is never silently overwritten.

## Exit codes

| Code | Class | A script should |
|:-----|:------|:----------------|
| `0` | converged | continue |
| `1` | data | stop — a human decides whose content is right |
| `2` | misuse | fix the invocation |
| `3` | transport | retry the job later — resume is free |

Post-retry per-artifact failures are transport failures: the summary lists the names, and the process exits `3` so a scheduler can distinguish "the pipe is down" from "the data is wrong".

## In NDJSON

Failures appear in the summary's `failed` list:

```json
{"event":"summary","uploaded":0,"downloaded":0,"skipped":1,"failed":["app.zip"]}
```

The human-readable error still goes to stderr.
The process exit code carries the class.

## In the Rust API

The same taxonomy is `nexus_raw_core::Error`:

```rust
match result {
    Err(e) if e.exit_code() == 3 => warn!("transport, retry later: {e}"),
    Err(e) => error!("data problem: {e}"),
    Ok(summary) => info!("{summary:?}"),
}
```

`Error::exit_code()` is the single mapping from error to exit code — wrappers call it instead of re-deriving one.
