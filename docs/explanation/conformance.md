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

## The suites

```bash
just test                                         # everything: units + conformance
cargo test -p nexus-raw-core --test conformance   # the facade against the mock
cargo test -p nexus-raw --test cli                # the binary against the mock
```

The core suite drives the facade directly and pins transfer semantics.
The CLI suite runs the real `nxr` binary over HTTP against the same mock and pins the user-visible surface on top: exit codes (0/1/2/3), the `hint:` line on stderr, `down` refusing without an enumeration source, `--plan` writing nothing, NDJSON event shapes, `doctor` verdicts.
Every scenario in the table is driven by the CLI suite at least once.

Core suite, by intent:

- **up**: `two_phase_up_recovers_after_partial_put`, `up_generates_markers_by_default`, `up_no_sha_skips_markers`, `markerless_remote_is_re_uploaded`, `second_up_is_all_skip`, `broken_local_marker_refuses_up`, `up_manifest_missing_local_name_is_data_error`, `up_claim_first_puts_the_claim_before_any_payload`, `up_claim_first_refuses_a_name_outside_the_scan`, `empty_dir_refuses_up`, `single_call_recovers_through_flaky`, `rate_limited_up_recovers_in_one_invocation`, `retry_events_name_the_object_they_retry`
- **down**: `down_fetches_and_writes_local_marker`, `down_resumes_from_part_with_range`, `down_resume_of_complete_part_finalizes_without_refetch`, `down_auto_enumerates_through_manifest_convention`, `down_digest_mismatch_refuses_and_drops_part`, `down_missing_name_is_data_error`, `down_markerless_remote_writes_computed_marker`, `down_stale_part_self_heals_without_a_flag`, `cut_body_down_completes_through_the_part`, `fake_length_down_recovers_through_a_short_read`
- **mirror**: `mirror_pours_byte_equal_trees_through_a_manifest`, `second_mirror_skips_with_zero_content_gets`, `mirror_refuses_diverged_destination_untouched`, `mirror_recovers_through_flaky`, `mirror_resumes_staged_part_with_range`, `mirror_completes_a_markerless_source`, `mirror_skips_when_a_markerless_source_matches_a_complete_destination`, `mirror_missing_name_is_data_error`, `mirror_uses_the_destination_client_for_its_writes`
- **rm and point**: `rm_removes_a_whole_version`, `second_rm_is_all_404_and_exits_clean`, `readonly_refuses_rm_and_changes_nothing`, `rm_dry_run_plans_and_touches_nothing`, `rm_without_names_refuses_with_enumeration_error`, `point_clear_is_idempotent`, `point_clear_on_readonly_refuses`
- **group**: `down_through_group_serves_the_first_member_holding_the_object`, `down_through_group_skips_a_failing_member_without_a_client_retry`, `up_against_a_group_refuses_with_405`, `rm_against_a_group_refuses_as_read_only`
- **auth, stall, transport and wave 2**: `auth_gates_every_request`, `auth_403_surfaces_as_the_auth_error`, `redirect_refuses_reads_with_http_301`, `stalled_download_retries_then_refuses`, `stalled_upload_fails_within_the_stall_window`, `drop_connection_is_retried_through_the_client`, `sizeless_head_refuses_instead_of_overwriting`, `divergent_complete_refusal_is_wire_covered`
- **verify, diff, channel, primitive, safety**: `verify_reports_local_state_without_network`, `diff_plan_is_deterministic_and_events_carry_lists`, `channel_set_get_and_forward_guard`, `channel_set_repairs_a_garbage_file`, `get_primitive_resumes_with_range`, `oversized_small_get_refuses_with_the_cap`, `put_sha_refuses_a_url_whose_marker_cannot_parse`, `symlinked_part_is_never_followed_on_resume`, `symlinked_marker_is_never_followed_on_write`, `symlinked_local_marker_is_never_followed_on_up`

CLI suite, by intent:

- **transfer through argv**: `up_down_roundtrip_atomic`, `up_generates_markers_by_default`, `up_no_sha_skips_markers`, `second_up_is_a_pure_skip`, `put_get_roundtrip_with_sha_sibling`, `head_reports_status`, `get_resumes_from_part_with_range`, `drop_connection_up_recovers`, `partial_put_up_recovers`, `cut_body_down_completes_through_the_part`, `slow_down_succeeds`, `flaky_up_recovers_in_one_invocation`, `doc_drift_down_takes_the_drifted_document`
- **enumeration**: `down_without_enumeration_needs_a_source`, `down_explicit_name`, `down_manifest_from_local_file`, `rm_without_a_source_refuses`
- **mirror**: `mirror_pours_the_version_and_reruns_are_pure_skips`, `mirror_refuses_a_diverged_destination`
- **rm and point**: `rm_removes_whole_version_and_reruns_clean`, `rm_readonly_refuses_and_keeps_bytes`, `rm_dry_run_touches_no_bytes`, `rm_json_events_parse_and_summarize`, `point_clear_roundtrip_and_readonly`
- **planning and observability**: `up_plan_prints_actions_without_uploading`, `down_plan_prints_plan_without_writing`, `ndjson_events_parse_and_summarize`, `rate_limit_up_retries_and_succeeds`, `failing_up_still_flushes_ndjson_events`
- **diff**: `diff_equal_directory_exits_zero`, `diff_reports_the_sections_and_writes_nothing`, `diff_without_enumeration_needs_a_source`, `diff_misuse_exits_two`
- **golden ndjson**: `golden_ndjson_up_down_hold`, `golden_ndjson_diff_holds`, `golden_ndjson_mirror_holds`, `golden_ndjson_retrying_holds`, `golden_ndjson_doctor_holds`
- **verification and pointers**: `verify_accepts_then_rejects_tampering`, `channel_set_get_and_if_forward`
- **exit codes and doctor**: `auth401_exit_codes` (3), `auth403_exit_3` (3), `redirect_exit_3` (3), `unsafe_name_is_misuse_exit_2` (2), `dead_base_exit_3` (3), `rm_exit_matrix_rows` (2 and 3), `diff_misuse_exits_two` (2), `doctor_exit_codes` (0 and 2), `doctor_json_lines`
- **scenario coverage**: `sizeless_up_refuses_instead_of_overwriting`, `markerless_down_writes_computed_marker`, `foreign_marker_down_refuses`

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
