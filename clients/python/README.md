# The Python reference client

`nxr.py` is the second client of the protocol: a minimal reference implementation on the Python standard library, python3.10+, nothing to install.
It proves the claim the project makes about its own wire format: the protocol is implementable from scratch over bare HTTP, with `http.client` for the wire and `hashlib` for the digests.
The authoritative text it follows lives in `crates/nexus-raw-core/src/protocol.md`.

## What it covers

Six verbs over the store: `get`, `put`, `head`, `sha`, `ls`, `channel get/set`.
Deletion is deliberately absent.
The interesting rules are all client-side and all present:

- `put` writes the bytes, then the `.sha256` marker of the same name, strictly in that order.
  A crash between the two leaves a markerless object, which every reader refuses.
- `get` downloads the bytes and the sibling and refuses unless the digest matches.
  Completion is bytes plus the sibling: anything less is an exception, never a silent return.
- A 2xx without `Content-Length` is present-but-unverifiable, never absent (`head` reports `length=None`).
- 401/403 fail fast with no retries; 5xx and transport failures retry with backoff (4 attempts, 0.5s x 2^n, capped, jittered); a 429 honors its `Retry-After` pause, clamped to 1..=60 seconds, in place of the backoff.
- Redirects are never followed: a 301 surfaces as `HttpError` with the status.
- The per-operation socket timeout is the stall watchdog: a frozen server aborts the attempt, a slow-but-moving transfer never trips it.
- `ls` walks `/service/rest/v1/search/assets` with continuation tokens, capped at 100 pages, and diagnoses a repository-scoped 400 as "missing or not raw".
- Markers parse strictly (`sha256sum -c` shape: LF only, exactly two spaces, 64 lowercase hex, trailing newline), because a loose parse would bless foreign markers.

Every error is an exception carrying `.status` (the HTTP status, `None` for pure transport failures) and `.hint` (the action line a CLI would print).
Every attempt lands in `.requests` as a `(method, path)` pair, the record the conformance suite asserts on.

## Run the conformance suite

```bash
cargo build -q -p mock-nexus
python3 -m unittest discover clients/python
```

The suite spawns `mock-nexus` itself, one process per test on a free port, and reads the scenario list live from `mock-nexus --print-scenarios`.
To point it at another binary, set `MOCK_NEXUS_BIN` to the path.

## The scenario contract and the skip table

The mock's scenario list is the conformance contract, and the suite refuses to drift from it:
`test_unit_every_scenario_is_classified` requires every listed scenario to have a test method named `test_<scenario with dashes as underscores>` or a documented row in `SKIP`, in both directions.
A scenario added to `crates/mock-nexus/src/scenario.rs` fails this suite until it is classified, which is the point.

The current skip rows, with the reasons:

| Scenario | Reason |
|:---------|:-------|
| `doc-drift` | The drift toggle is a library-only knob (`MockNexus::enable_drift`), so the binary serves plain storage; the client-side half, `version.json` being an ordinary name, is pinned under `atomic` |
| `no-service` | The scenario only breaks `/service/rest/v1/repositories`, and the client has no service-repos surface |
| `readonly` | The scenario only answers 403 to `DELETE`, and the client implements no delete |

## Porting note

What a real integrator must implement, at minimum, beyond the verbs:

1. Basic auth attached to every request, resolved once per invocation.
2. The retry classification: retry transport failures and 5xx with bounded backoff, honor `Retry-After` on 429, never retry 401/403, never follow redirects.
3. An attempt-level stall bound (a read/write timeout), because a server can hold a connection forever.
4. The marker order on upload, the digest check on download, and the strict marker parse.
5. The lengthless-answer rule: a 2xx without `Content-Length` is unverifiable, never empty.
6. `Accept-Encoding: identity` on every request, unless the client also implements the `zstd` GET decoding, so digests always describe the stored bytes.

Deliberate simplifications of this reference, safe to outgrow: bodies are held in memory (no streaming, no Range resume, no part files), one connection per attempt (no keep-alive), and TLS is whatever the runtime verifies by default (the protocol's `--tls-insecure` escape hatch has no equivalent here).
