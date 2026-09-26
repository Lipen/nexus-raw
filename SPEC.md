# SPEC: nexus-raw v0.3 (curl for Nexus raw)

Status: implemented by this repository.
The docs site carries the guides; this file is the normative summary.
A divergence between this text and the code is a bug in the code.

---

## 0. One phrase

nxr is curl for a Nexus raw repository: HTTP-grade primitives (get/put/head/sha) with retries, stall detection and TLS on by default, verified directory transfers on top (many files, parallel, Range-resume, digest checks), helpers for versions and channel refs, one static binary, zero config files.

## 1. Model

### 1.1 Store shape

```text
<base>/<name>            artifact bytes
<base>/<name>.sha256     sha-sibling marker: "<hex>  <name>\n" (sha256sum -c)
<base>/manifest.json     conventional name list (the enumeration source for down)
<channel-url>            a token file: "<token>\n" — any name, any location
```

- `<base>` is any directory URL inside a raw repository.
- A name is a relative path; segments match `[A-Za-z0-9._-]+` (1..=255 bytes, no `.`/`..`); the `.sha256` suffix is reserved.
- An object is **complete** = bytes + sibling with a matching digest.
- No claim protocol, no reserved pointer names: `claim.json`, `latest`, `nightly`, `manifest.json` are ordinary artifact names. A `manifest.json` inside an uploaded directory is uploaded like any file.

### 1.2 States

Local and remote objects classify as:

| State | Meaning |
|:------|:--------|
| `Complete(digest)` | bytes + valid sibling, digest matches |
| `Markerless` | bytes without a sibling |
| `Broken(reason)` | sibling present but unparseable, or digest diverges |
| `Absent` | no bytes |

`Broken` is always refused, never overwritten.

## 2. Layers

Dependencies point one way: layers never import upward.

| Layer | Scope | Code |
|:------|:------|:-----|
| L0 transport | GET/HEAD/PUT, streaming, retries (4 attempts, 0.5s×2ⁿ + jitter ≤ 250ms), stall detection (30s default), TLS verification, auth, Range requests | `transport/` |
| L0 primitives | `get`, `put`, `head`, `sha` — curl-grade single-object operations, no verification, no classification | `primitive.rs` |
| L1 transfer | scan → symmetric diff → parallel plan execution; markers; Range-resume; refusals | `sync/` |
| L2 layout | channels, manifests, search-based listings — conventions, not protocol | `layout/` |
| L3 UX | CLI, doctor, progress rendering, NDJSON, hints | `crates/nexus-raw` |

## 3. Primitives (L0)

- `nxr get <URL> [-o FILE] [--continue]` — stream to file or stdout.
  File mode writes `<out>.part` and renames on success; `--continue` resumes from the part through `Range: bytes=N-` (a `200` answer restarts from zero).
  Stdout mode is a single body attempt: a retry after the body started would duplicate bytes.
- `nxr put <URL> -f FILE [--sha]` — streamed upload with Content-Length; `--sha` hashes the file and PUTs `<url>.sha256` right after the bytes. Without `--sha` no marker is written.
- `nxr head <URL>` — status, size, content type; 404 (and 401) are results, not errors.
- `nxr sha <FILE|URL>` — streaming sha256, output compatible with `sha256sum`.

Primitives never classify, never diff, never refuse to overwrite.

## 4. Transfer (L1)

### 4.1 up

`nxr up <SRC_DIR> <DST_URL> [--manifest FILE|URL|-] [--no-sha] [--dry-run]`

- Enumeration: a recursive scan of the source directory (hidden segments and `*.sha256` skipped); `--manifest` restricts the transfer to the listed names and is a data error (exit 1) when a listed name does not exist locally.
- **Markers are mandatory by default.** Missing local siblings are generated (streaming hash + canonical marker file); existing siblings are verified by the classification; a broken sibling refuses the whole up (exit 1). After a successful up the local directory is self-complete.
- `--no-sha` opts out: bytes are uploaded, no marker is written, locally the sibling is not generated. Hashing still happens where a divergence check needs it.
- Classification is symmetric (§4.3); every `Upload` action is `PUT bytes` → `PUT marker` in that order.
- Idempotent: a rerun skips everything complete and retries the rest.
- Refusals (exit 1, nothing overwritten): complete objects diverging on digest; broken markers on either side; unverifiable-on-both-sides (Markerless × Markerless).

### 4.2 down

`nxr down <SRC_URL> <DST_DIR> [--manifest FILE|URL|-] [--name NAME]... [--ls] [--continue]`

#### 4.2.1 Enumeration (decisive)

Nexus raw has no guaranteed directory listing. `down` therefore requires an enumeration source; without one it refuses (exit 1) with a hint:

| Source | Flag | Guarantee |
|:-------|:-----|:----------|
| server manifest (default) | — | `GET <dir-url>manifest.json`; refuses when absent |
| explicit manifest | `--manifest FILE\|URL\|-` | exact list (file, URL, or stdin) |
| explicit names | `--name NAME` (repeatable) | exact list |
| best-effort search | `--ls` | `/service/rest/v1/search/assets`; refuses with a hint when the server lacks it |

Recommended convention: a channel resolves to a version; the version carries `manifest.json`; `down` enumerates from it. `up` publishes the manifest as an ordinary file, which closes the loop.

#### 4.2.2 Fetch

- Bytes stream into a stable part file (`.nxr-part-<hash>`); `--continue` resumes from its size through a Range request (206), a `200` answer restarts.
- The downloaded digest is verified against the remote sibling when one exists; on divergence the part is deleted — a corrupt prefix never survives to be resumed.
- On success: rename bytes into place, write the local sibling. An interrupted name leaves only the part file.

### 4.3 Symmetric diff

| local \ remote | Complete | Markerless | Absent |
|:---------------|:---------|:-----------|:-------|
| **Complete** | digests equal → Skip; diverge → refuse | up: Upload (finish the partial); down: Skip | up: Upload; down: Skip |
| **Markerless** | up: compare hashes (equal → Skip, diverge → refuse); down: Download | refuse | up: Upload (hash computed); down: Missing |
| **Broken** | refuse | refuse | refuse |
| **Absent** | up: refuse (up never deletes); down: Download | up: refuse; down: Download (digest computed after) | Missing |

`Missing` fires only when no other refusal exists. `Download` of a Markerless remote computes the marker locally instead of trusting anything.

## 5. Layout helpers (L2)

- **Channels**: a channel ref is any object holding exactly one token line (`<token>\n`, no CR).
  `nxr channel get <URL>` prints it; `nxr channel set <URL> <TOKEN> [--if-forward]` writes it.
  `--if-forward` keeps the current token when it already compares `>=` the new one (dotted-numeric: `1.10.0 > 1.9.9`, `1.0.0 > 1.0.0-rc1`). An unreadable current value never blocks a write.
- **Manifests**: `{"artifacts": ["<name>", ...]}` — duplicates removed, every name through the grammar. Claim-shaped fields are tolerated (`claim_version` must be 1 when present; `version` ignored).
- **Listings**: `nxr ls <URL>` lists versions under a repository/group URL; `nxr ls <URL> --assets` lists objects under a directory URL. Both go through the search API and are experimental: every failure is a refusal with a hint, never a silent empty list.

## 6. CLI contracts

- Exit codes: 0 ok, 1 data (mismatch, missing, enumeration, digest divergence), 2 misuse (bad flags, unsafe names, empty dir), 3 transport/auth. clap arg errors also exit 2.
- Every error prints `error: <message>` plus a `hint: <what to check>` line on stderr. The hint travels in JSON output as `hint`.
- `--json` emits NDJSON events (`plan`, `artifact`, `summary` for transfers; one JSON object for head/put/sha/channel/doctor). Event shapes are fixed by golden tests.
- Progress: coalesced byte events (≤ 1 per 200ms per artifact); `-q` keeps the summary; `-v` adds starts and plan lines.
- `nxr verify <DIR> [--manifest FILE|-]`: local bytes + marker + digest, no network; strict `Complete` verdict.
- `nxr doctor [URL]`: resolved credential source (never values), TLS state, settings bounds, optional reachability probe. Failures exit 3 (network) or 2 (local); success exits 0.

## 7. Credentials and configuration

- There is **no config file** and there are **no profiles**. Every call names its URL.
- Credential precedence: `-u user:pass` (curl parity, deliberate argv exposure — doctor warns) → `NXR_AUTH` (base64 `user:pass`) → `NXR_USERNAME` + `NXR_PASSWORD` (must be set together). Anonymous otherwise.
- Credentials never appear in logs, JSON output, or temp files.

## 8. Transport guarantees

- TLS verified by default; `--tls-insecure` is explicit and per-invocation.
- Retries: `--retry 4` attempts per request with 0.5s×2ⁿ backoff + jitter; 4xx never retried; 5xx and connection errors are.
- Stall: no bytes for `--stall-secs 30` fails the attempt (retryable). Uploads detect a full outbound channel; downloads a silent stream.
- Requests are idempotent-or-resumable: PUT retries resend from scratch; GET retries restart the attempt and the part file absorbs the progress.

## 9. Non-goals

Config files and profiles; server-side components; DELETE; archive extraction; project-specific conventions (nightly policies, consumer layouts); anything requiring a daemon.

## 10. Compatibility

- v0.2 claim-protocol clients are not served: claims are read only as manifests (the `artifacts` array), never enforced as immutability contracts.
- Marker files stay `sha256sum -c`-compatible: `<lowercase-hex>  <name>\n`.
- Future additions (zstd content-encoding, byte-range PUT, `nxr delete`) must be additive and move this document's version.
