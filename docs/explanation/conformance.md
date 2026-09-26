# Conformance

A client for a storage protocol is only as good as its failure handling, so the failure modes are first-class fixtures.
This page maps every fixture to the invariant it pins and to the test that pins it.
The design reasoning behind these guarantees lives in [Design](design.md), the wire behavior they approximate in the [protocol reference](../reference/protocol.md).

## The mock and the one table

`crates/mock-nexus` is a std-only HTTP/1.1 server implementing exactly enough of the protocol to exercise the transport contract: `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, resumable GET through `Range: bytes=N-` (a `206` with `Content-Range`, a `416` on out-of-range starts), and Basic-auth gating.
A scenario selects a failure mode.
The scenario list is one table — the Rust suites, the mock binary and any external harness all read the same list, so it never forks.

## The matrix

Every scenario, the invariant it pins, and the tests that pin it.
`atomic` is the control group: it is what the server does when nothing goes wrong, so most semantics tests run against it.

| Scenario | The server does | Invariant pinned | Pinned by |
|:---------|:----------------|:-----------------|:----------|
| `atomic` | straight-through storage, `Range` resume with `206`/`416`, 404 on unknown paths | the happy path: marker defaults, pure skips on replay, part resume, enumeration, the exit-code surface | core: 15 tests and CLI: 15 tests below · mock: `atomic_roundtrip_put_get_head`, `get_range_serves_suffix_body_and_rejects_out_of_bounds` |
| `partial-put` | the first PUT body is cut short, the connection closes, nothing is stored | a retried attempt completes the upload and stores it whole | core: `two_phase_up_recovers_after_partial_put` · mock: `partial_put_first_attempt_cut_then_stores` |
| `drop-connection` | the first request per path is answered with a TCP reset | a reset is retryable like any broken transport | mock: `drop_connection_resets_first_request_per_path` — the client-side retry loop that recovers from resets is the same one pinned through `partial-put` and `flaky` |
| `slow` | bodies arrive in small delayed chunks | stall detection aborts the attempt, the retry loop continues, and an honest refusal is the last resort | core: `stalled_download_retries_then_refuses` · mock: `slow_get_roundtrip` |
| `foreign-marker` | stored markers carry a foreign digest | a remote `Broken` object is never overwritten — the digest check refuses and the part is dropped | core: `down_digest_mismatch_refuses_and_drops_part` · mock: `foreign_marker_zeroes_stored_digest` |
| `markerless` | markers are acknowledged with `201` but silently dropped | a Markerless remote is re-uploaded by `up`, and `down` writes the marker it computed from the received bytes | core: `markerless_remote_is_re_uploaded`, `down_markerless_remote_writes_computed_marker` · mock: `markerless_loses_marker_keeps_bytes` |
| `auth-401` | `401` without valid Basic credentials | auth failures fail fast with no retries and name the URL | core: `auth_gates_every_request` · CLI: `auth401_exit_codes` · mock: `auth_401_gate` |
| `claim-drift` | after `enable_drift`, every GET of `*/claim.json` serves a synthesized claim with a ghost artifact | `claim.json` is an ordinary name — a legacy fixture kept as a trap for future special-casing and as a target for external harnesses | mock: `claim_drift_synthesizes_between_enable_and_disable` |
| `flaky` | the first K requests per path answer `503` | one invocation recovers through retries | core: `single_call_recovers_through_flaky_and_dropped_connections` · mock: `flaky_serves_503_for_first_k_requests` |

## The suites

```bash
just test                                         # everything: units + conformance
cargo test -p nexus-raw-core --test conformance   # 22 tests, the facade against the mock
cargo test -p nexus-raw --test cli                # 18 tests, the binary against the mock
```

The core suite drives the facade directly and pins transfer semantics.
The CLI suite runs the real `nxr` binary over HTTP against the same mock and pins the user-visible surface on top: exit codes (0/1/2/3), the `hint:` line on stderr, `down` refusing without an enumeration source, `--dry-run` writing nothing, NDJSON event shapes, `doctor` verdicts.

Every test of the core suite (22), by intent:

- **up** — `two_phase_up_recovers_after_partial_put` (a cut PUT is completed by the retry), `up_generates_markers_by_default` (missing local siblings are generated before upload), `up_no_sha_skips_markers` (the explicit downgrade leaves Markerless objects), `markerless_remote_is_re_uploaded` (a Markerless remote is repaired, bytes then marker), `second_up_is_all_skip` (a completed upload replays as pure skips), `broken_local_marker_refuses_up` (Broken local refuses, never uploads), `up_manifest_missing_local_name_is_data_error` (a manifest naming a missing file is data, not silence), `single_call_recovers_through_flaky_and_dropped_connections` (retries converge inside one invocation), `empty_dir_refuses_up` (nothing to upload is misuse, not an empty success)
- **down** — `down_fetches_and_writes_local_marker` (bytes land, then the local marker from received bytes), `down_resumes_from_part_with_range` (a partial part continues through `206`), `down_resume_of_complete_part_finalizes_without_refetch` (the `416` edge finalizes without a range fetch), `down_auto_enumerates_through_manifest_convention` (`manifest.json` at the directory URL enumerates), `down_digest_mismatch_refuses_and_drops_part` (a wrong part is deleted and refused), `down_missing_name_is_data_error` (a name the server lacks is data), `down_markerless_remote_writes_computed_marker` (no remote sibling — hash locally, write the marker)
- **auth and stall** — `auth_gates_every_request` (a `401` surfaces as failure without retries), `stalled_download_retries_then_refuses` (stall timeout, retry, then honest refusal)
- **verify, diff, channel, primitive** — `verify_reports_local_state_without_network` (local classification needs no server), `diff_plan_is_deterministic_and_events_carry_lists` (same input, same plan), `channel_set_get_and_forward_guard` (token refs and the rollback guard), `get_primitive_resumes_with_range` (the L0 primitive resumes a `.part` through `206`)

Every test of the CLI suite (18), by intent:

- **transfer through argv** — `up_down_roundtrip_atomic`, `up_generates_markers_by_default`, `up_no_sha_skips_markers`, `second_up_is_a_pure_skip`, `put_get_roundtrip_with_sha_sibling`, `head_reports_status`, `get_resumes_from_part_with_range`
- **enumeration** — `down_without_enumeration_needs_a_source`, `down_explicit_name`, `down_manifest_from_local_file`
- **planning and observability** — `dry_run_prints_plan_without_uploading`, `ndjson_events_parse_and_summarize`
- **verification** — `verify_accepts_then_rejects_tampering`
- **pointers** — `channel_set_get_and_if_forward`
- **exit codes** — `auth401_exit_codes` (1), `unsafe_name_is_misuse_exit_2` (2), `dead_base_exit_3` (3), `doctor_exit_codes` (0 and 2)

## The counts

22 conformance tests through the core facade, 18 CLI tests through the real binary, 30 unit tests in the core library, plus doctests.
The mock carries its own suite pinning the behavior table itself.
The rule that keeps this honest: a scenario added for any client lands in the same PR as the test that needs it.
The scenario list never forks — the Rust suites, the mock binary and any future external suites read the same table.

## Driving the mock by hand

```bash
just mock atomic --port 8080
cargo run -p mock-nexus -- slow --chunk-delay-ms 2000 --port 8080
cargo run -p mock-nexus -- auth-401 --auth ci:secret --port 8080
```

It prints its address and serves until killed — point any client at it, including ones this project does not know about.
For the failure-to-symptom path from a consumer's chair, see [when it breaks](../how-to/troubleshoot.md), and for the error taxonomy the [errors and exit codes](../reference/errors.md) reference.
