# Wire invariants

This page is the failure-scenario contract for external clients of the protocol.
It lists every scenario the `mock-nexus` fixture serves, what it does on the wire, and the wire invariant each scenario pins.
The content is generated from the scenario table in `crates/mock-nexus/src/scenario.rs` (`SCENARIOS`), the same list `mock-nexus --print-scenarios` prints as JSON.
A scenario added or changed there must land in the same change as this page: the drift test in the `mock-nexus` crate fails until the page is regenerated with `UPDATE_INVARIANTS=1 cargo test -p mock-nexus invariant`.
Run one scenario with `just mock <id>` (or `cargo run -p mock-nexus -- <id>`) and point any client at the printed address.
A group repository is a separate deployment kind, not a scenario.
The per-suite test matrix and the design reasoning: [conformance](../explanation/conformance.md).

## `atomic`

Straight-through storage, the control group.

Every fully-read request is served normally.
PUT stores the bytes and answers `201`, GET serves the stored bytes and HEAD their length, DELETE removes an existing object with `204` and answers `404` on an absent one, and unknown paths answer `404`.
A GET honors resumable downloads: a single open `Range: bytes=N-` is answered with `206` and `Content-Range`, an out-of-range start with `416`.

**Pinned invariant:** The happy path of the protocol, the baseline every other scenario deviates from on one axis.

## `partial-put`

Truncated first upload.

The first PUT per path is cut off after `first_attempt_bytes` body bytes: the connection closes with no response and nothing is stored.
Later PUT attempts on that path are read fully and stored.

**Pinned invariant:** A cut attempt stores nothing, and a retried attempt completes the upload with the object stored whole.

## `cut-body`

Truncated first download.

The first success (200) GET per path writes honest status and headers, then only `after_bytes` body bytes and closes the connection.
With `--fake-length` the `Content-Length` header lies 1024 bytes high, so the honest-looking answer still breaks mid-body.
Later GETs serve the object whole, so a retry resuming from the part file completes the download.

**Pinned invariant:** A mid-body break is a retryable transport failure: the retry resumes from the part file and the download lands byte-perfect, with or without the lying header.

## `drop-connection`

Connection reset after the request head.

The first request per path (any method) is reset right after its head (request line and headers) is read: the body is never read, nothing is answered and nothing is stored.
Later requests are served normally.

**Pinned invariant:** A reset before any answer is retryable like any broken transport.

## `freeze-upload`

Held upload.

PUT connections are held right after the head: the body is never read and no response is ever written, so the write side must detect the stall.
The hold is bounded (about 15 minutes), then the connection is dropped.
GET and HEAD behave like `atomic`.

**Pinned invariant:** A stalled upload is aborted by the attempt watchdog instead of hanging forever.

## `sizeless`

Success answers without Content-Length.

Success (200) GET and HEAD answers for present objects carry no `Content-Length`: the proxy case.
A client must refuse to treat such objects as absent (the digest comparison would be skipped).
Every other answer (206, 416, 404) keeps `Content-Length`.
PUTs behave like `atomic`.

**Pinned invariant:** A 2xx answer without a length is a broken answer, never an absent object: the client refuses instead of overwriting.

## `slow`

Slow body.

GET and HEAD bodies are written in `chunk_size` pieces, sleeping `chunk_delay_ms` between pieces.
PUT bodies are read normally.

**Pinned invariant:** Stall detection aborts the slow attempt, the retry loop continues, and an honest refusal is the last resort.

## `foreign-marker`

Foreign digest in the marker.

Stored `.sha256` markers get their 64-char digest replaced by 64 zeros (the digest of a foreign object).
Everything else is stored verbatim.

**Pinned invariant:** A remote object whose sibling belongs to a foreign object is never overwritten: the digest check refuses and the partial file is dropped.

## `markerless`

Markers silently dropped.

`.sha256` markers are acknowledged (201 Created) but never stored.
Non-marker bytes are stored normally.

**Pinned invariant:** Marker symmetry survives a markerless server: `up` re-uploads the markers and `down` writes the marker it computed from the received bytes.

## `auth-401`

Basic auth challenge (401).

Every request requires `Authorization: Basic base64(user:pass)`.
Otherwise the answer is `401` with `WWW-Authenticate: Basic realm="nexus"`.
Valid credentials behave like `atomic`.

**Pinned invariant:** Auth failures fail fast with no retries, and the failure names the URL.

## `auth-403`

Basic auth refusal (403).

Every request requires `Authorization: Basic base64(user:pass)`.
Anything else answers `403 Forbidden` with a `forbidden` body, on any method (unlike `auth-401`, which answers `401`).
Valid credentials behave like `atomic`.

**Pinned invariant:** A 403 refusal maps onto the auth error class exactly like a 401 challenge.

## `doc-drift`

Version document drift.

Behaves like `atomic` until drift is enabled through the library API (`MockNexus::enable_drift`).
Afterwards every GET of a `*/version.json` path serves a synthesized version document with a ghost artifact.
PUTs keep storing verbatim, and `MockNexus::disable_drift` restores store-backed responses.

**Pinned invariant:** A version document is an ordinary name with no protocol meaning: the client serves what the server serves instead of special-casing it.

## `flaky`

503 burst.

The first `first_failures` requests per path (any method) get `503 Service Unavailable`.
Later requests are served normally.

**Pinned invariant:** One invocation recovers a 503 burst through retries.

## `rate-limit`

429 with Retry-After.

The first `first_429s` requests per path (any method) answer `429 Too Many Requests` with a `Retry-After: <seconds>` header and an empty body.
Later requests are served like `atomic`.

**Pinned invariant:** The client honors the `Retry-After` pause, spends attempts on the 429s and still lands the transfer in one invocation.

## `redirect`

Redirect on reads.

GET and HEAD requests answer `301 Moved Permanently` with `Location: <location_path>` (same host, the path from the repository root) and an empty body.
Every other method behaves like `atomic`.

**Pinned invariant:** Redirects are never followed: a 301 on a read surfaces as a plain transport error.

## `readonly`

Read-only repository.

Every DELETE is refused with `403 Forbidden`: the read-only repository.
Nothing is ever removed from the store.
GET, HEAD and PUT behave like `atomic`.

**Pinned invariant:** A refused deletion changes nothing: the client reports the refusal and a rerun stays safe.

## `no-service`

No service REST endpoint.

The service REST endpoint (`/service/rest/v1/repositories`) answers `404` like a store miss: an installation without the management API (an old Nexus, or a non-Sonatype server).
Storage behavior is `atomic`; only `service repos` notices.

**Pinned invariant:** The service REST endpoint is optional: the client refuses with a hint that names the server root, and every storage command is unaffected.
