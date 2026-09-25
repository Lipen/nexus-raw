# Wire protocol

What `nxr` actually says to the server: layouts, objects, the diff classification and the write order.
The implementation treats this text as the contract — a divergence is a bug.

## Layout

A base URL points inside a raw repository, at a directory that holds versions and pointers:

```text
<base>/
├── latest                              # pointer: "<version>\n"
├── nightly                             # pointer
└── <version>/
    ├── claim.json                      # immutable artifact list
    └── <name>[.sha256]                 # artifacts and their markers
```

Example: base `https://nexus.example.com/repository/raw-main/`, version `1.4.0`, artifact `bom/linux-x86_64.json`:

```text
GET https://nexus.example.com/repository/raw-main/1.4.0/bom/linux-x86_64.json
```

Only three methods exist: `GET`, `HEAD`, `PUT`.
There is no delete, no rename, no server-side computation.

## Objects

### claim.json

```json
{"claim_version": 1, "version": "1.4.0", "artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json"]}
```

Written once, before the artifacts, and never rewritten.
It names the version's artifacts.
It does not carry digests — each artifact's marker does.
A reader refuses claims whose `claim_version` is not `1`, whose version fails the segment grammar, or that name artifacts failing the name grammar.

### sha-sibling

Exactly one strict `sha256sum -c` line next to the bytes:

```text
<64 lowercase hex chars>␠␠<name>\n
```

The digest covers the artifact bytes, not the marker.
An artifact is **complete** when bytes and marker both exist and the digest matches.

### pointer

One line: `<version>\n`.
The only mutable state besides version directories.
Writes are atomic PUTs.

## Names

- A name is a relative path: segments through `/`.
- A segment matches `[A-Za-z0-9._-]+`, 1..=255 bytes.
- Forbidden: empty segments, `.`/`..`, leading or trailing `/`.
- Reserved: the name `claim.json`, the pointer names `latest` and `nightly`, the `.sha256` suffix.

## States

| State | Bytes | Marker | Condition |
|:------|:------|:-------|:----------|
| `Complete` | yes | yes | digest matches |
| `Markerless` | yes | no | under-uploaded or mid-publish |
| `Broken` | yes/no | yes | digest mismatch, or the marker does not parse |
| `Absent` | no | — | bytes decide, and a stray marker is ignored |

## The diff

For every claim name, the local and remote states are compared symmetrically — the same classification drives `up` and `down`:

| Local | Remote | Action |
|:------|:-------|:-------|
| `Complete` | `Complete`, equal digests | skip |
| `Complete` | `Complete`, different digests | **mismatch — refuse** |
| `Complete` | `Absent` / `Markerless` | upload (up) |
| `Markerless` | `Complete` | download — the remote marker is the truth |
| `Markerless` | `Markerless` | **refuse** — nothing to verify against |
| `Markerless` | `Absent` | **missing** — no complete copy anywhere |
| `Broken` | anything | **mismatch — refuse, never overwrite** |
| `Absent` | `Complete` | download |
| `Absent` | `Markerless` | download — the local marker is computed from the received bytes |
| `Absent` | `Absent` | **missing** — claimed but exists nowhere |

Remote state costs two requests per name: `HEAD` on the bytes and `GET` on the sibling, overlapped by the worker pool.

## The write order

```text
1. PUT claim.json            — if present and byte-equal: ok; if different: claim drift, refuse
2. PUT <name>                — bytes, in parallel workers
3. PUT <name>.sha256         — strictly after the bytes of the same name
```

Marker-after-bytes is what makes `Markerless` mean "under-uploaded": a client never trusts a marker whose bytes are absent, and a crash between steps 2 and 3 is recoverable by design.
Downloads mirror the order into the destination: bytes into a temp file (hashed on the fly), digest check, rename, then the marker.

## Transport

| Aspect | Behavior |
|:-------|:---------|
| auth | `Basic`, attached to every request when credentials resolve |
| TLS | verified by default (`--tls-insecure` is the only off-switch) |
| retries | up to 4 attempts per request, retrying connect errors, timeouts, body breaks and 5xx — never 4xx |
| backoff | 0.5 s × 2ⁿ + jitter ≤ 250 ms |
| stall | no bytes for `--stall-secs` aborts the attempt (default 30 s) |
| timeouts | connect timeout only, no total-per-artifact timeout — a big artifact on a slow link is legitimate |
| parallelism | 8 workers by default, `--workers 1..=64` |
| idempotency | PUTs are byte-exact repeats, and a resumed transfer replays the same bytes |

## Errors

The taxonomy mirrors these guarantees.
The mapping from error to exit code has one home — see [errors and exit codes](errors.md).
