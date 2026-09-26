# Conformance

A client for a storage protocol is only as good as its failure handling, so the failure modes are first-class fixtures.

## The scenario table

The mock server (`crates/mock-nexus`) implements one behavior table.
The Rust conformance suites drive the exact same list from both sides: the core facade directly, and the real `nxr` binary over HTTP.

| Scenario | Behavior | What the client must survive |
|:---------|:---------|:-----------------------------|
| `atomic` | correct PUT/GET/HEAD, `Range: bytes=N-` resume (206, 416 out-of-range), 404 on unknown paths | the happy path, including downloads resumed from a part |
| `partial-put` | the first PUT body is cut short, connection closed | a retried attempt completes the upload |
| `drop-connection` | the first request per path gets a connection reset | retry from scratch |
| `slow` | response bodies arrive in small delayed chunks | stall detection and honest transport errors |
| `foreign-marker` | stored markers carry a foreign digest | refuse with `mismatch`, never overwrite |
| `markerless` | markers are accepted but dropped | re-upload on the next diff and keep marker-after-bytes order |
| `auth-401` | 401 without valid Basic credentials | fail fast, no retries, name the URL |
| `claim-drift` | diverges `GET */claim.json` once enabled (legacy fixture) | the client treats `claim.json` as an ordinary name |
| `flaky` | the first K requests per path answer 503 | recover within one invocation through retries |

## The suites

```bash
just test                          # everything: units + conformance
cargo test -p nexus-raw-core --test conformance   # 21 tests, facade against the mock
cargo test -p nexus-raw --test cli                # 18 tests, the binary against the mock
```

The core suite pins the transfer semantics: marker generation by default, `--no-sha` opting out, markerless remote re-upload, resume from `.part` and `.nxr-part-<hash>` through 206, refusal tables, channel forward-guards, auth gating, stall recovery.
The CLI suite pins the surface on top: exit codes (0/1/2/3), the `hint:` line on stderr, `down` refusing without an enumeration source, `--dry-run` writing nothing, NDJSON event shapes, `doctor` verdicts.

## Driving the mock by hand

```bash
just mock atomic --port 8080
cargo run -p mock-nexus -- slow --chunk-delay-ms 2000 --port 8080
cargo run -p mock-nexus -- auth-401 --auth ci:secret --port 8080
```

It prints its address and serves until killed — point any client at it, including the ones this project does not know about.

## The one-list rule

A new failure scenario enters `crates/mock-nexus` (the `SCENARIOS` list) **in the same change** as the client code that needs it.
The scenario list never forks: Rust suites, the mock binary and any future external suites all read the same table.
