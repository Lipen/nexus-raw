# nexus-raw-core


The core of nexus-raw: curl for a Sonatype Nexus raw repository.
The entry point is the [`Nxr`] facade.
The CLI and wrappers are thin and carry no protocol logic.

## Layers

- **L0 transport + primitives**: [`transport`] (retries, stall, TLS, auth)
and [`primitive`] (`get`/`put`/`head`/`sha`), curl-grade, no verification.
- **L1 transfer**: [`sync`], directory up/down and repository-to-repository mirror
with the symmetric diff, sha-sibling markers, parallel workers and Range-resume
(`.part` files).
- **L2 layout helpers**: [`layout`], channels (token refs with any name),
search-based listings.
- **L3 UX** lives in the CLI: doctor, logs, hints, NDJSON.

## The shape of a store

```text
<base>/<name>             artifact bytes
<base>/<name>.sha256      sha-sibling: "<hex>  <name>\n" (sha256sum -c)
<channel-url>             a token file: "<token>\n", any name
```

A name is complete = bytes + sibling with a matching digest.
Local/remote states: `Complete`, `Markerless` (bytes without a marker),
`Broken` (marker mismatches or does not parse), `Absent`.
The symmetric diff decides what to transfer.
`Mismatch` and `Missing` refuse without overwriting.

`up` writes markers by default.
`--no-sha` opts out.
`down` requires an enumeration source: explicit names, or the best-effort search API.

`mirror` moves enumerated names between two repositories through a staging directory derived from both bases.
Two concurrent `mirror` runs over one pair would interleave `.part` writes, so the staging directory holds a lock: the second run refuses with a hint (misuse, exit 2) and touches nothing.
The lock is portable: unix guards the lock file with an exclusive `flock`, windows opens it with file sharing denied, so any second open fails while the first run lives.
The handle is the lock: the file is never unlinked while held, and a crashed run leaves nothing to clean up.

`diff --via search` is a read-side helper, not a new wire format.
The remote side of the delta report comes from the search API's per-asset facts (name, digest, size) instead of a HEAD/sibling pair per name, so a big tree is compared in O(pages) requests.
A searched asset that lacks a search-reported digest counts as changed, never equal: there is nothing to verify against.
A search refusal (400 for an unknown repository, 404 for a missing endpoint) degrades honestly: the run refuses with the enumeration-family error, or with `--fallback` it announces the degradation and rebuilds the report from the probe-based enumeration.

Deletion (`rm`, `point --clear`) is the one deliberate extension (§5.4).
DELETE is allowed for the objects of the enumerated version only: bytes, markers,
the version document, the pointer file.
The order is the reverse of publishing: the marker first, then the bytes,
so nobody ever sees a complete object mid-delete.
404 is a normal answer: deletion is idempotent.
A 403/405 is the read-only repository: refused with a hint, exit 1.
Divergence is never checked when deleting: names, not content.

Errors: [`Error`] with [`Error::exit_code`] (0 ok, 1 data, 2 misuse, 3 transport) and [`Error::hint`], the human hint the CLI renders.
A storage 404 (the object or version is absent) is data: exit 1 with the `Http { status: 404 }` error, never transport.
A service-API 404 (`service repos` against a server without the management API) stays transport: exit 3 with [`Error::ServiceMissing`].

The transfer worker pool is adaptive (§7 retries).
A directory transfer (`up`, `down`, `mirror`, and `mv`'s pour) starts with as many workers as `--workers` grants.
When a retryable request fails with a `Retry-After` (a 429), the effective limit halves at once: the run stops acquiring new workers past the reduced limit, and permits still held are surrendered as their transfers finish.
The floor is 1 worker: some transfer always goes on.
On a streak of successes (one step per four consecutive successes) the limit recovers one worker at a time, never exceeding the configured maximum.
A single success after a 429 does not restore the limit: the recovery slope must fill first.
The wire behavior is unchanged: only how many transfers run at once.

## Range and segmented reads

The wire contract is one open span per request.
A GET with `Range: bytes=N-` that the server honors answers `206` with `Content-Range` and the suffix body.
A span that starts at or past the object's end answers `416` with `Content-Range: bytes */<total>`.
Any other `Range` form is the server's business: the client never sends it on the resume path.

Segmented download (`get --segments N`) is a client-side strategy on top of that contract, not a wire extension.
The client HEADs the URL for `Content-Length`, splits the object into N contiguous closed spans (`bytes=start-end`), fetches them concurrently, and assembles the answers positionally into one file that the usual digest check and atomic rename complete.
A server that answers a ranged GET with the whole body (`200`) ignores `Range`: the client detects the full-body answer and degrades to the single-stream download, logged through the events channel, never an error.
Any segment that fails after retries fails the whole download: the segment files stay in the staging area for inspection, and nothing partial is ever renamed to the output name.
