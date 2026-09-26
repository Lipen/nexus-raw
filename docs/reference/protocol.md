# Wire protocol

What `nxr` actually says to the server: the store layout, objects, the classification that drives transfers, and the write order.
This page documents implemented behavior.
The normative protocol text is maintained by the project outside the public docs, and a divergence between code and that text is a bug in the code.

## The shape of a store

A base URL points at a directory inside a raw repository.
Everything `nxr` writes fits four object kinds:

```text
<base>/<name>            artifact bytes
<base>/<name>.sha256     sha-sibling: "<hex>  <name>\n" (sha256sum -c format)
<base>/manifest.json     conventional name list — the enumeration source for down
<base>/<channel>         a token file: "<token>\n", any name (latest, stable, …)
```

Only three methods exist: `GET`, `HEAD`, `PUT`.
There is no delete, no rename, no server-side computation.

Example: base `https://nexus.example.com/repository/raw-main/`, artifact `bom/linux-x86_64.json`:

```text
GET https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json
```

## Objects

### sha-sibling

Exactly one strict `sha256sum -c` line next to the bytes:

```text
<64 lowercase hex chars>␠␠<name>\n
```

The digest covers the artifact bytes, not the marker.
An artifact is **complete** when bytes and sibling both exist and the digest matches.
The sibling is the marker of its bytes, never an artifact of its own — `nxr` never uploads a `.sha256` as a name in its own right.

### manifest

```json
{"artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json"]}
```

An ordinary file with a conventional role: the enumeration source for `down` and the optional name filter for `up`.
It carries no digests — each artifact's sibling does.
Names may appear in any order, duplicates are dropped.
Claim-shaped fields are tolerated: `claim_version` must be `1` when present, `version` is ignored.
`manifest.json` sitting in a published directory is just a file that `up` uploads like any artifact.

### channel ref

One line: `<token>\n`, written by `channel set`.
A channel is **any** name — `latest`, `stable`, `prod-1` are all ordinary token files.
Dotted-numeric tokens compare in version order for `--if-forward` (`1.4.0` < `1.10.0`, because segments compare numerically).
The only mutable state besides version directories, and writes are plain PUTs.

## Names

- A name is a relative path: segments through `/`.
- A segment matches `[A-Za-z0-9._-]+`.
- Forbidden: empty segments, `.`/`..`, leading or trailing `/`.
- Reserved: the `.sha256` suffix — nothing else.
- `claim.json`, `latest`, `nightly` are ordinary names: upload them, download them, point a channel at them.
  What a name *means* is the publisher's convention, not the protocol's.

## States

For every name, each side (local directory or remote directory) is in one of:

| State | Bytes | Marker | Condition |
|:------|:------|:-------|:----------|
| `Complete` | yes | yes | digest matches |
| `Markerless` | yes | no | bytes without a marker — under-uploaded or mid-transfer |
| `Broken` | yes | yes | digest mismatch, or the marker does not parse |
| `Absent` | — | — | no bytes, a stray marker is ignored |

## The symmetric diff

The same classification drives `up` and `down`.
The mode only decides what a single completed copy means: send it (up), fetch it (down), or keep it.

| Local | Remote | Action |
|:------|:-------|:-------|
| `Complete` | `Complete`, equal digests | skip |
| `Complete` | `Complete`, different digests | **mismatch — refuse** |
| `Complete` | `Absent` / `Markerless` | up: upload · down: skip — the local copy is the truth |
| `Markerless` | `Complete` | up: hash and compare, then skip or refuse · down: download |
| `Markerless` | `Markerless` | **mismatch — refuse, nothing to verify against** |
| `Markerless` | `Absent` | up: hash and upload · down: **missing** — no completed copy anywhere |
| `Broken` (local) | anything | **mismatch — refuse, never overwrite** |
| anything | `Broken` (remote sibling) | **mismatch — refuse, a foreign marker is never overwritten** |
| `Absent` | `Complete` / `Markerless` | up: **mismatch — up never deletes** · down: download |
| `Absent` | `Absent` | **missing** |

`Missing` is collected across all names and fires only when no other refusal exists.
A refusal never transfers, never overwrites, and exits `1`.

Remote state costs two requests per name: `HEAD` on the bytes and `GET` on the sibling, overlapped by the worker pool.

## The write order

Upload:

```text
1. generate missing local siblings      — hash the bytes, write canonical markers
2. PUT <name>                           — bytes, in parallel workers
3. PUT <name>.sha256                    — strictly after the bytes of the same name
```

Marker-after-bytes is what makes `Markerless` mean "under-uploaded": a client never trusts a marker whose bytes are absent, and a crash between steps 2 and 3 is recoverable by design.
`--no-sha` opts out of steps 1 and 3 on purpose — the result is Markerless objects the server never certifies.

Download mirrors the order locally: bytes stream into a stable part file (`.nxr-part-<hash>`) hashed on the fly, the digest is checked against the remote sibling when one exists, the part renames into place, and the local sibling is written from the received bytes.
A diverging digest deletes the part file.

## Resume

Range requests are the only recovery mechanism, and they need no server cooperation beyond HTTP:

- `get --continue` resumes `<out>.part` with `Range: bytes=N-` — a `206` appends from the part, a `200` (range ignored) restarts from zero, anything else is an error.
- `down --continue` resumes each name's `.nxr-part-<hash>` the same way — the part name is a stable hash of the artifact name, so a repeated run finds its fuel.
- Uploads cannot resume: a PUT is byte-exact and replayed whole, which is safe because PUTs are idempotent.

## Enumeration

`up` never needs to enumerate: it scans the local directory.
`down` must learn the name list from an explicit source, because Nexus raw has no guaranteed directory listing:

| Source | Flag | Guarantee |
|:-------|:-----|:----------|
| manifest at the directory URL | *(default)* | exact, when `manifest.json` exists there |
| manifest file, URL or stdin | `--manifest` | exact |
| explicit names | `--name` (repeatable) | exact |
| server search API | `--ls` | best-effort, depends on the server release |

Without any source and without `manifest.json` at the directory URL, `down` refuses with `cannot enumerate` and a hint — no guessing, no HEAD-probing for likely names.

## Transport

| Aspect | Behavior |
|:-------|:---------|
| auth | `Basic`, attached to every request when credentials resolve — `-u` beats `NXR_AUTH` beats `NXR_USERNAME`+`NXR_PASSWORD` |
| TLS | verified by default (`--tls-insecure` is the only off-switch) |
| retries | up to 4 attempts per request, retrying connect errors, timeouts, body breaks and 5xx — never 4xx |
| backoff | 0.5 s × 2ⁿ + jitter ≤ 250 ms |
| stall | no bytes for `--stall-secs` aborts the attempt (default 30 s) |
| timeouts | connect timeout only, no total-per-artifact timeout — a big artifact on a slow link is legitimate |
| parallelism | 8 workers by default (`--workers`) |
| idempotency | PUTs are byte-exact repeats, and a resumed download replays the same bytes |

## Errors

The taxonomy mirrors these guarantees.
The mapping from error to exit code has one home — see [errors and exit codes](errors.md).
