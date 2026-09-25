# Conformance

A client for a storage protocol is only as good as its failure handling, so the failure modes are first-class fixtures.

## The scenario table

The mock server (`crates/mock-nexus`) implements one behavior table.
The Rust conformance suites and external test suites (a Python client, a CI job) drive the exact same list.

| Scenario | Behavior | What the client must survive |
|:---------|:---------|:-----------------------------|
| `atomic` | correct PUT/GET/HEAD behavior | the happy path |
| `partial-put` | the first PUT body is cut short, connection closed | a retried attempt completes the upload |
| `drop-connection` | the first request per path gets a connection reset | retry from scratch |
| `slow` | response bodies arrive in small delayed chunks | stall detection and honest transport errors |
| `foreign-marker` | stored markers carry a foreign digest | refuse with `mismatch`, never overwrite |
| `markerless` | markers are accepted but dropped | re-upload on the next diff and keep marker-after-bytes order |
| `auth-401` | 401 without valid Basic credentials | fail fast, no retries, name the URL |
| `claim-drift` | `GET claim.json` diverges once drift is enabled | refuse with `claim drift`, touch nothing |
| `flaky` | the first K requests per path answer 503 | recover within one invocation through retries |

## Running the suite

```bash
just test                          # everything: units + conformance
cargo test -p nexus-raw-core --test conformance
cargo test -p nexus-raw --test cli
```

The CLI suite drives the real `nxr` binary against the in-process mock.
The core suite drives the facade directly.

## Driving the mock by hand

```bash
just mock atomic --port 8080
cargo run -p mock-nexus -- slow --chunk-delay-ms 2000 --port 8080
cargo run -p mock-nexus -- auth-401 --auth ci:secret --port 8080
```

It prints its address and serves until killed — point any client at it, including the ones this project does not know about.

## The one-list rule

A new failure scenario enters `crates/mock-nexus` (the `SCENARIOS` list) **in the same change** as the client code that needs it.
Implementations in other languages regenerate their fixtures from that list, so "the table" never forks.
