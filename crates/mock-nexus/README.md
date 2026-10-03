# mock-nexus

A mock Sonatype Nexus raw server with a failure-scenario table.
It is the shared conformance fixture: the Rust test suites, a human debugging a client and external test suites all drive the exact same list of scenarios.

## Run

```bash
cargo run -p mock-nexus -- atomic --port 8080
# prints: listening http://127.0.0.1:8080
```

Point any client at the printed address — `nxr`, your own code, a curl loop.

## Scenarios

| Scenario | Behavior |
|:---------|:---------|
| `atomic` | the correct server: PUT/GET/HEAD, DELETE with 204/404, `Range: bytes=N-` resume (206/416), 404 on unknown paths |
| `partial-put` | cuts the first PUT body short and closes the connection |
| `drop-connection` | resets the first request per path after reading its headers |
| `freeze-upload` | accepts a PUT connection and then never reads or answers, so the client's stall detection must fire |
| `sizeless` | omits `Content-Length` on GET responses (chunked/streamed bodies of unknown size) |
| `slow` | writes response bodies in small delayed chunks |
| `foreign-marker` | stores markers with a foreign digest |
| `markerless` | accepts markers but silently drops them |
| `auth-401` | requires `Authorization: Basic <base64 user:pass>` |
| `doc-drift` | diverges `GET version.json` once drift is enabled by the test |
| `flaky` | answers 503 for the first K requests per path |
| `readonly` | answers 403 to every DELETE: the read-only repository, the store never shrinks |

Scenario-specific flags: `--partial-bytes N`, `--chunk-delay-ms N`, `--chunk-size N`, `--flaky K`, `--auth user:pass` (default `ci:secret`).

## As a library

The same server is a lib, so Rust tests start it in-process:

```rust
use mock_nexus::{MockNexus, Scenario};

let server = MockNexus::start(Scenario::Flaky { first_failures: 2 })?;
// ... run the client against server.base_url(), assert on server.requests()
```

The `SCENARIOS` list in `src/scenario.rs` is the conformance contract: a new scenario lands in the same change as the client behavior that needs it.

A group repository is a separate deployment kind, not a scenario: `MockNexus::start_group(&[&first, &second])?` builds one over two or more running members.
Reads are forwarded to the members in order and the first `2xx` is relayed, writes are refused with `405` and `Allow: GET,HEAD`.

## More

- User-facing documentation: [docs/](../../docs/)
- The conformance matrix in action: [docs/explanation/conformance](../../docs/explanation/conformance.md)
