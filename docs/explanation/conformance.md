# Conformance

A client for a storage protocol is only as good as its failure handling, so the failure modes are first-class fixtures.
This page maps every fixture to the invariant it pins and to the test that pins it.
The design reasoning behind these guarantees lives in [Design](design.md), the wire behavior they approximate in the [protocol reference](../reference/protocol.md).

## The mock and the one table

`crates/mock-nexus` is a std-only HTTP/1.1 server implementing exactly enough of the protocol to exercise the transport contract: `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, resumable GET through `Range: bytes=N-` (a `206` with `Content-Range`, a `416` on out-of-range starts), and Basic-auth gating.
A scenario selects a failure mode.
The scenario list has one home: the `Scenario` enum in `crates/mock-nexus/src/scenario.rs`, with each mode's exact behavior in its doc comment.
The Rust suites, the mock binary (`mock-nexus --print-scenarios`) and any external harness all read that one list, so it never forks.

## The matrix

Every scenario, the invariant it pins, and the tests that pin it.
`atomic` is the control group: it is what the server does when nothing goes wrong, so most semantics tests run against it.

| Scenario | The server does | Invariant pinned | Pinned by |
|:---------|:----------------|:-----------------|:----------|
| `atomic` | straight-through storage, `Range` resume with `206`/`416`, 404 on unknown paths, DELETE with 204 on existing and 404 on missing | the happy path: marker defaults, pure skips on replay, part resume, enumeration, deletion and its idempotency, the exit-code surface | most transfer tests in both suites below · mock: `atomic_roundtrip_put_get_head`, `atomic_delete_removes_then_404s`, `get_range_serves_suffix_body_and_rejects_out_of_bounds` |
| `partial-put` | the first PUT body is cut short, the connection closes, nothing is stored | a retried attempt completes the upload and stores it whole | core: `two_phase_up_recovers_after_partial_put` · mock: `partial_put_first_attempt_cut_then_stores` |
| `drop-connection` | the first request per path is answered with a TCP reset | a reset is retryable like any broken transport | core: `drop_connection_is_retried_through_the_client` · mock: `drop_connection_resets_first_request_per_path` |
| `freeze-upload` | the PUT connection is held right after the head: the body is never read and no response is ever written | the attempt-level stall watchdog aborts a frozen upload instead of hanging | core: `stalled_upload_fails_within_the_stall_window` |
| `sizeless` | GET/HEAD answers for present objects carry no `Content-Length` | a 2xx without a length is `Broken`: refused, never treated as `Absent` | core: `sizeless_head_refuses_instead_of_overwriting` |
| `slow` | bodies arrive in small delayed chunks | stall detection aborts the attempt, the retry loop continues, and an honest refusal is the last resort | core: `stalled_download_retries_then_refuses` · mock: `slow_get_roundtrip` |
| `foreign-marker` | stored markers carry a foreign digest | a remote `Broken` object is never overwritten: the digest check refuses and the part is dropped | core: `down_digest_mismatch_refuses_and_drops_part` · mock: `foreign_marker_zeroes_stored_digest` |
| `markerless` | markers are acknowledged with `201` but silently dropped | a Markerless remote is re-uploaded by `up`, and `down` writes the marker it computed from the received bytes | core: `markerless_remote_is_re_uploaded`, `down_markerless_remote_writes_computed_marker`, `mirror_completes_a_markerless_source` · mock: `markerless_loses_marker_keeps_bytes` |
| `auth-401` | `401` without valid Basic credentials | auth failures fail fast with no retries and name the URL | core: `auth_gates_every_request` · CLI: `auth401_exit_codes` · mock: `auth_401_gate` |
| `doc-drift` | after `enable_drift`, every GET of `*/version.json` serves a synthesized version document with a ghost artifact | `version.json` is an ordinary name; the legacy fixture is kept as a trap for future special-casing and as a target for external harnesses | mock: `doc_drift_synthesizes_between_enable_and_disable` |
| `flaky` | the first K requests per path answer `503` | one invocation recovers through retries | core: `single_call_recovers_through_flaky` · mock: `flaky_serves_503_for_first_k_requests` |
| `readonly` | every DELETE answers `403`, the store keeps everything | the read-only repository refuses `rm` and `point --clear` with exit 1, and a refused run has deleted nothing | core: `readonly_refuses_rm_and_changes_nothing`, `point_clear_on_readonly_refuses` · CLI: `rm_readonly_refuses_and_keeps_bytes` · mock: `readonly_refuses_every_delete` |

## The suites

```bash
just test                                         # everything: units + conformance
cargo test -p nexus-raw-core --test conformance   # the facade against the mock
cargo test -p nexus-raw --test cli                # the binary against the mock
```

The core suite drives the facade directly and pins transfer semantics.
The CLI suite runs the real `nxr` binary over HTTP against the same mock and pins the user-visible surface on top: exit codes (0/1/2/3), the `hint:` line on stderr, `down` refusing without an enumeration source, `--dry-run` writing nothing, NDJSON event shapes, `doctor` verdicts.

Core suite, by intent:

- **up**: `two_phase_up_recovers_after_partial_put`, `up_generates_markers_by_default`, `up_no_sha_skips_markers`, `markerless_remote_is_re_uploaded`, `second_up_is_all_skip`, `broken_local_marker_refuses_up`, `up_manifest_missing_local_name_is_data_error`, `up_claim_first_puts_the_claim_before_any_payload`, `up_claim_first_refuses_a_name_outside_the_scan`, `empty_dir_refuses_up`, `single_call_recovers_through_flaky`, `retry_events_name_the_object_they_retry`
- **down**: `down_fetches_and_writes_local_marker`, `down_resumes_from_part_with_range`, `down_resume_of_complete_part_finalizes_without_refetch`, `down_auto_enumerates_through_manifest_convention`, `down_digest_mismatch_refuses_and_drops_part`, `down_missing_name_is_data_error`, `down_markerless_remote_writes_computed_marker`, `down_stale_part_self_heals_without_a_flag`
- **rm and point**: `rm_removes_a_whole_version`, `second_rm_is_all_404_and_exits_clean`, `readonly_refuses_rm_and_changes_nothing`, `rm_dry_run_plans_and_touches_nothing`, `rm_without_names_refuses_with_enumeration_error`, `point_clear_is_idempotent`, `point_clear_on_readonly_refuses`
- **auth, stall and transport**: `auth_gates_every_request`, `stalled_download_retries_then_refuses`, `stalled_upload_fails_within_the_stall_window`, `drop_connection_is_retried_through_the_client`, `sizeless_head_refuses_instead_of_overwriting`, `divergent_complete_refusal_is_wire_covered`
- **verify, diff, channel, primitive, safety**: `verify_reports_local_state_without_network`, `diff_plan_is_deterministic_and_events_carry_lists`, `channel_set_get_and_forward_guard`, `get_primitive_resumes_with_range`, `oversized_small_get_refuses_with_the_cap`, `symlinked_part_is_never_followed_on_resume`, `symlinked_marker_is_never_followed_on_write`, `symlinked_local_marker_is_never_followed_on_up`

CLI suite, by intent:

- **transfer through argv**: `up_down_roundtrip_atomic`, `up_generates_markers_by_default`, `up_no_sha_skips_markers`, `second_up_is_a_pure_skip`, `put_get_roundtrip_with_sha_sibling`, `head_reports_status`, `get_resumes_from_part_with_range`
- **enumeration**: `down_without_enumeration_needs_a_source`, `down_explicit_name`, `down_manifest_from_local_file`, `rm_without_a_source_refuses`
- **rm and point**: `rm_removes_whole_version_and_reruns_clean`, `rm_readonly_refuses_and_keeps_bytes`, `rm_dry_run_touches_no_bytes`, `rm_json_events_parse_and_summarize`, `point_clear_roundtrip_and_readonly`
- **planning and observability**: `dry_run_prints_plan_without_uploading`, `ndjson_events_parse_and_summarize`, `failing_up_still_flushes_ndjson_events`, `golden_ndjson_up_down_hold`, `golden_ndjson_doctor_holds`
- **verification and pointers**: `verify_accepts_then_rejects_tampering`, `channel_set_get_and_if_forward`
- **exit codes and doctor**: `auth401_exit_codes` (3), `unsafe_name_is_misuse_exit_2` (2), `dead_base_exit_3` (3), `rm_exit_matrix_rows` (2 and 3), `doctor_exit_codes` (0 and 2), `doctor_json_lines`

## The counts

41 conformance tests through the core facade, 29 CLI tests through the real binary, 35 unit tests in the core library, plus doctests.
The mock carries its own suite of 12 tests pinning the behavior table itself.
The rule that keeps this honest: a scenario added for any client lands in the same PR as the test that needs it, in `crates/mock-nexus/src/lib.rs`, so the list never forks.

## Driving the mock by hand

```bash
just mock atomic --port 8080
cargo run -p mock-nexus -- slow --chunk-delay-ms 2000 --port 8080
cargo run -p mock-nexus -- auth-401 --auth ci:secret --port 8080
```

It prints its address and serves until killed: point any client at it, including ones this project does not know about.
For the failure-to-symptom path from a consumer's chair, see [when it breaks](../how-to/troubleshoot.md), and for the error taxonomy the [errors and exit codes](../reference/errors.md) reference.
