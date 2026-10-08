# Conformance

A client for a storage protocol is only as good as its failure handling, so the failure modes are first-class fixtures.
This page maps every fixture to the invariant it pins and to the test that pins it.
The design reasoning behind these guarantees lives in [Design](design.md), the wire behavior they approximate in the [protocol reference](../reference/protocol.md).

## The mock and the one table

`crates/mock-nexus` is a std-only HTTP/1.1 server implementing exactly enough of the protocol to exercise the transport contract: `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, resumable GET through `Range: bytes=N-` (a `206` with `Content-Range`, a `416` on out-of-range starts), and Basic-auth gating.
A scenario selects a failure mode.
The scenario list has one home: the `Scenario` enum in `crates/mock-nexus/src/scenario.rs`, with each mode's exact behavior in its doc comment.
The Rust suites, the mock binary (`mock-nexus --print-scenarios`) and any external harness all read that one list, so it never forks.

The mock keeps its own answers honest: a status answer that can reach a HEAD (401, 403, 405, 503) carries the body's `Content-Length` but no body bytes (RFC 9110 §9.3.2), and a request head past the 64 KiB bound is answered with 400 instead of a silent hang-up.

Group repositories are a separate deployment kind, not a scenario: `MockNexus::start_group` builds one over two or more running members.
The group forwards reads to the members in order and relays the first `2xx`, so every scenario in the table above keeps applying behind a group.
Writes are refused by the group itself with `405` and `Allow: GET,HEAD`, in the wire shape of the real Nexus group handler.
The dispatch is faithful to the real `GroupHandler.java` (method switch at lines 103-112, the first-2xx member walk at 124-166).
Group search is not modeled: the search API reads the metadata database rather than the group dispatch.

## The matrix

Every scenario, the invariant it pins, and the tests that pin it.
The client-facing contract page rendered from the same table: [wire invariants](../reference/invariants.md).
`atomic` is the control group: it is what the server does when nothing goes wrong, so most semantics tests run against it.

| Scenario | The server does | Invariant pinned | Pinned by |
|:---------|:----------------|:-----------------|:----------|
| `atomic` | straight-through storage, `Range` resume with `206`/`416`, 404 on unknown paths, DELETE with 204 on existing and 404 on missing | the happy path: marker defaults, pure skips on replay, part resume, enumeration, deletion and its idempotency, the exit-code surface | most transfer tests in both suites below · mock: `atomic_roundtrip_put_get_head`, `atomic_delete_removes_then_404s`, `get_range_serves_suffix_body_and_rejects_out_of_bounds` |
| `partial-put` | the first PUT body is cut short, the connection closes, nothing is stored | a retried attempt completes the upload and stores it whole | core: `two_phase_up_recovers_after_partial_put` · CLI: `partial_put_up_recovers` · mock: `partial_put_first_attempt_cut_then_stores` |
| `cut-body` | the first success GET per path ships honest status and headers, then a truncated body and a close; `fake-length` lies the declared `Content-Length` 1024 bytes high | a mid-body break is a retryable transport failure: the retry resumes from the part and the download lands byte-perfect, lie or no lie | core: `cut_body_down_completes_through_the_part`, `fake_length_down_recovers_through_a_short_read` · CLI: `cut_body_down_completes_through_the_part` · mock: `cut_body_first_get_truncated_then_honest`, `cut_body_fake_length_lies_high` |
| `drop-connection` | the first request per path is reset right after its request line and headers are read: the body is never read, nothing is answered or stored | a reset is retryable like any broken transport | core: `drop_connection_is_retried_through_the_client` · CLI: `drop_connection_up_recovers` · mock: `drop_connection_resets_first_request_per_path` |
| `freeze-upload` | the PUT connection is held right after the head: the body is never read and no response is ever written (the hold is bounded, then the connection drops) | the attempt-level stall watchdog aborts a frozen upload instead of hanging | core: `stalled_upload_fails_within_the_stall_window` · CLI: `failing_up_still_flushes_ndjson_events` |
| `sizeless` | success (200) GET and HEAD answers for present objects carry no `Content-Length`; 206, 416 and misses keep it | a 2xx without a length is `Broken`: refused, never treated as `Absent` | core: `sizeless_head_refuses_instead_of_overwriting` · CLI: `sizeless_up_refuses_instead_of_overwriting` |
| `slow` | bodies arrive in small delayed chunks | stall detection aborts the attempt, the retry loop continues, and an honest refusal is the last resort | core: `stalled_download_retries_then_refuses` · CLI: `slow_down_succeeds` · mock: `slow_get_roundtrip` |
| `foreign-marker` | stored markers carry a foreign digest | a remote `Broken` object is never overwritten: the digest check refuses and the part is dropped | core: `down_digest_mismatch_refuses_and_drops_part` · CLI: `foreign_marker_down_refuses` · mock: `foreign_marker_zeroes_stored_digest` |
| `markerless` | markers are acknowledged with `201` but silently dropped | a Markerless remote is re-uploaded by `up`, and `down` writes the marker it computed from the received bytes | core: `markerless_remote_is_re_uploaded`, `down_markerless_remote_writes_computed_marker`, `mirror_completes_a_markerless_source` · CLI: `markerless_down_writes_computed_marker` · mock: `markerless_loses_marker_keeps_bytes` |
| `auth-401` | `401` with a `WWW-Authenticate` challenge without valid Basic credentials | auth failures fail fast with no retries and name the URL | core: `auth_gates_every_request` · CLI: `auth401_exit_codes` · mock: `auth_401_gate` |
| `auth-403` | `403` with a `forbidden` body without valid Basic credentials, on any method | a 403 maps onto the auth error (exit 3) exactly like a 401 | core: `auth_403_surfaces_as_the_auth_error` · CLI: `auth403_exit_3` · mock: `auth_403_gate` |
| `doc-drift` | after `enable_drift`, every GET of `*/version.json` serves a synthesized version document with a ghost artifact | `version.json` is an ordinary name; the legacy fixture is kept as a trap for future special-casing and as a target for external harnesses | CLI: `doc_drift_down_takes_the_drifted_document` · mock: `doc_drift_synthesizes_between_enable_and_disable` |
| `flaky` | the first K requests per path answer `503` | one invocation recovers through retries | core: `single_call_recovers_through_flaky` · CLI: `flaky_up_recovers_in_one_invocation` · mock: `flaky_serves_503_for_first_k_requests` |
| `rate-limit` | the first N requests per path answer `429` with a `Retry-After` header and an empty body, then serve like atomic | the client honors the `Retry-After` pause, spends attempts on the 429s and still lands the transfer in one invocation | core: `rate_limited_up_recovers_in_one_invocation` · CLI: `rate_limit_up_retries_and_succeeds`, `golden_ndjson_retrying_holds` · mock: `rate_limit_serves_429_then_atomic` |
| `redirect` | GET and HEAD answer `301` with `Location` and an empty body; writes stay atomic | redirects are never followed: a 301 surfaces as the plain HTTP error (exit 3) | core: `redirect_refuses_reads_with_http_301` · CLI: `redirect_exit_3` · mock: `redirect_answers_301_on_reads_only` |
| `readonly` | every DELETE answers `403`, the store keeps everything | the read-only repository refuses `rm` and `point --clear` with exit 1, and a refused run has deleted nothing | core: `readonly_refuses_rm_and_changes_nothing`, `point_clear_on_readonly_refuses` · CLI: `rm_readonly_refuses_and_keeps_bytes` · mock: `readonly_refuses_every_delete` |
| `no-service` | the service REST endpoint answers `404` like a store miss; storage is `atomic` | `service repos` refuses with exit 3 and the server-root hint; every storage invariant is unaffected | core: `service_repos_missing_refuses_with_the_root_hint` · CLI: `service_repos_missing_prints_the_root_hint` · mock: the seeded document is simply absent |
| `search-400` | the search API answers `400` when the `repository` parameter names an unknown repository; served repositories and other requests behave like `atomic` | a repository-scoped search 400 is diagnosed: `ls` refuses with exit 3 and the missing-or-non-raw-repository hint, and a search 404 keeps the generic enumeration hint | core: `ls_unknown_repository_hinted_on_400_not_on_404` · CLI: `ls_unknown_repository_prints_the_repository_hint` · mock: `search_unknown_repo_answers_400_known_repos_pass_through` |

## The suites

```bash
just test                                         # everything: units + conformance
cargo test -p nexus-raw-core --test conformance   # the facade against the mock
cargo test -p nexus-raw --test cli                # the binary against the mock
```

The core suite drives the facade directly and pins transfer semantics.
The CLI suite runs the real `nxr` binary over HTTP against the same mock and pins the user-visible surface on top: exit codes (0/1/2/3), the `hint:` line on stderr, `down` refusing without an enumeration source, `--plan` writing nothing, NDJSON event shapes, `doctor` verdicts.
Every scenario in the table is driven by the CLI suite at least once, and the table's *Pinned by* column names the test for every fixture.

The rest of each suite pins semantics that do not belong to one scenario: misuse refusals (an empty scan, a name outside it, a missing enumeration source), replay purity, marker and part hygiene on the local disk, and the golden `--json` shapes.
The full inventory lives where it stays true: `crates/nexus-raw-core/tests/conformance.rs` and `crates/nexus-raw/tests/cli.rs`.
Names there are written to be read next to their assertions, not out of context.

## The second client

`clients/python` is the proof that the protocol stands on its own: a reference client on the Python standard library alone (`http.client`, `hashlib`, nothing to install), implementing `get`, `put`, `head`, `sha`, `ls` and the channel verbs from the protocol text.
Its conformance suite drives the `mock-nexus` binary over real HTTP, one process per test on a free port, the same matrix the Rust suites drive through the facade:

```bash
cargo build -q -p mock-nexus
python3 -m unittest discover clients/python
```

Because the binary has no in-process handle, the suite reads the store through plain retry-free requests and reads the request log from the client itself, which records every attempt as a `(method, path)` pair: the marker-ordering assertion is the same, one layer closer.
The retry policy is the Rust transport's, shrunk to one file: 4 attempts, 0.5s x 2^n backoff with jitter, a 429's `Retry-After` honored in place of the backoff, 401/403 failing fast, the per-operation socket timeout as the stall watchdog.
Scenarios outside the client's surface are skip-listed in the suite with the reason (`doc-drift` is a library-only knob, `no-service` and `readonly` guard surfaces the client does not have).

The drift rule lives in `test_unit_every_scenario_is_classified`:
the suite fetches the scenario list live from `mock-nexus --print-scenarios` and requires every listed scenario to have a test method named after it or a row in the skip table, in both directions.
A scenario added to `crates/mock-nexus/src/scenario.rs` fails the Python suite until it is classified, so a second implementation cannot fall behind the table.
That is what makes the list the conformance contract rather than a convention: a third client can lift the same mechanism, point it at the same `--print-scenarios`, and inherit the same stand.

## The composition

The core suite pins transfer semantics through the facade, the CLI suite pins the user-visible surface through the real binary, the mock carries its own suite pinning the behavior table itself (every scenario plus the group forwarding), and the golden fixtures pin the `--json` contract byte-exact.
The rule that keeps this honest: a scenario added for any client lands in the same PR as the test that needs it, in `crates/mock-nexus/src/scenario.rs`, so the list never forks.

## Driving the mock by hand

```bash
just mock atomic --port 8080
cargo run -p mock-nexus -- slow --chunk-delay-ms 2000 --port 8080
cargo run -p mock-nexus -- auth-401 --auth ci:secret --port 8080
cargo run -p mock-nexus -- rate-limit --rate-429s 3 --retry-after-secs 5 --port 8080
```

It prints its address and serves until killed: point any client at it, including ones this project does not know about.
For the failure-to-symptom path from a consumer's chair, see [when it breaks](../how-to/troubleshoot.md), and for the error taxonomy the [errors and exit codes](../reference/errors.md) reference.
