# Conformance

Failure handling decides whether a storage client works, so the mock server turns every failure mode into a fixture.
This page maps every fixture to the invariant it pins and to the test that pins it.
The design reasoning behind these guarantees lives in [Design](design.md), the wire behavior they approximate in the [protocol reference](../reference/protocol.md).

## The mock and the one table

`crates/mock-nexus` is a std-only HTTP/1.1 server implementing exactly enough of the protocol to exercise the transport contract: `GET`/`HEAD`/`PUT` with `Content-Length` or chunked bodies, resumable GET through `Range: bytes=N-` (a `206` with `Content-Range`, a `416` on out-of-range starts), and Basic-auth gating.
A scenario selects a failure mode.
The scenario list has one home: the `Scenario` enum in `crates/mock-nexus/src/scenario.rs`, with each mode's exact behavior in its doc comment.
The Rust suites, the mock binary (`mock-nexus --print-scenarios`) and any external harness all read that one list, so it never forks.

The mock also follows the protocol in its own answers: a status answer that can reach a HEAD (401, 403, 405, 503) carries the body's `Content-Length` but no body bytes (RFC 9110 §9.3.2), and a request head past the 64 KiB bound is answered with 400 instead of a silent hang-up.

<figure>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 760 320" role="img" aria-labelledby="dg-table-t dg-table-d" font-family="inherit">
<title id="dg-table-t">One scenario table, many drivers</title>
<desc id="dg-table-d">The Scenario enum in mock-nexus is the single source. The core suite, the CLI suite, the python suite and the mock binary all read or derive from it, and the python drift guard fails any scenario that lacks a test or a skip entry.</desc>
<style>
.dg-src { fill: color-mix(in oklch, var(--dg-accent) 10%, var(--md-default-bg-color)); stroke: var(--dg-accent); stroke-width: 1.5;
.dg-card2 { fill: var(--md-default-bg-color); stroke: var(--dg-line); stroke-width: 1.5;
.dg-ttl { font-size: 14px; font-weight: 700; fill: var(--dg-ink);
.dg-sub { font-size: 11.5px; fill: var(--dg-muted);
.dg-edge { font-size: 11px; fill: var(--dg-muted);
.dg-ln4 { stroke: var(--dg-muted); stroke-width: 1.5; fill: none;
.dg-ln4d { stroke: var(--dg-muted); stroke-width: 1.5; fill: none; stroke-dasharray: 6 4;
.dg-head4 { fill: var(--dg-muted);
.dg-plate4 {
  fill: var(--md-default-bg-color);;
}

</style>
<defs>
<marker id="dg-arrow4" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
<path d="M0,1 L9,5 L0,9 Z" class="dg-head4"/>
</marker>
</defs>
<!-- source card top-center: x=230..530, y=24..96 -->
<g filter="url(#dg-shadow4)">
<rect x="230" y="24" width="300" height="72" rx="12" class="dg-src"/>
</g>
<text x="380" y="50" text-anchor="middle" class="dg-ttl">scenario.rs · the Scenario enum</text>
<text x="380" y="70" text-anchor="middle" class="dg-sub">one list: every failure mode with its doc comment</text>
<text x="380" y="86" text-anchor="middle" class="dg-sub">the conformance contract</text>
<!-- driver cards y=200..272, centers x=120/320/520/680... grid: 4 cards of 160, gaps 20: total 700, x0=30 -->
<g filter="url(#dg-shadow4)">
<rect x="30" y="200" width="160" height="72" rx="12" class="dg-card2"/>
</g>
<text x="110" y="226" text-anchor="middle" class="dg-ttl">core suite</text>
<text x="110" y="246" text-anchor="middle" class="dg-sub">facade, in process</text>
<g filter="url(#dg-shadow4)">
<rect x="210" y="200" width="160" height="72" rx="12" class="dg-card2"/>
</g>
<text x="290" y="226" text-anchor="middle" class="dg-ttl">CLI suite</text>
<text x="290" y="246" text-anchor="middle" class="dg-sub">the real nxr binary</text>
<g filter="url(#dg-shadow4)">
<rect x="390" y="200" width="160" height="72" rx="12" class="dg-card2"/>
</g>
<text x="470" y="226" text-anchor="middle" class="dg-ttl">python suite</text>
<text x="470" y="246" text-anchor="middle" class="dg-sub">stdlib client, real HTTP</text>
<g filter="url(#dg-shadow4)">
<rect x="570" y="200" width="160" height="72" rx="12" class="dg-card2"/>
</g>
<text x="650" y="226" text-anchor="middle" class="dg-ttl">mock binary</text>
<text x="650" y="246" text-anchor="middle" class="dg-sub">--print-scenarios</text>
<!-- fan-out: from source bottom (y=96) to each card top (y=200), rounded orthogonal -->
<path d="M 330,96 L 330,132 Q 330,144 318,144 L 110,144 Q 110,144 110,156 L 110,200" class="dg-ln4" marker-end="url(#dg-arrow4)"/>
<path d="M 350,96 L 350,140 Q 350,152 338,152 L 290,152 Q 290,152 290,164 L 290,200" class="dg-ln4" marker-end="url(#dg-arrow4)"/>
<path d="M 410,96 L 410,140 Q 410,152 422,152 L 470,152 L 470,200" class="dg-ln4" marker-end="url(#dg-arrow4)"/>
<path d="M 430,96 L 430,132 Q 430,144 442,144 L 650,144 L 650,200" class="dg-ln4" marker-end="url(#dg-arrow4)"/>
<!-- drift guard: the python suite cross-checks itself against the source, dashed, bottom lane y=296, right lane x=752 -->
<path d="M 470,272 L 470,288 Q 470,296 482,296 L 740,296 Q 752,296 752,284 L 752,60 Q 752,52 744,52 L 530,52" class="dg-ln4d" marker-end="url(#dg-arrow4)"/>
<g transform="translate(610,296)"><rect x="-84" y="-9" width="168" height="18" class="dg-plate4"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">every scenario tested or skip-listed</text></g>
<text x="700" y="140" text-anchor="middle" class="dg-edge">drift guard</text>
<filter id="dg-shadow4" x="-10%" y="-10%" width="120%" height="135%">
<feGaussianBlur in="SourceAlpha" stdDeviation="3"/>
<feOffset dy="2"/>
<feComponentTransfer><feFuncA type="linear" slope="0.10"/></feComponentTransfer>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
</svg>
</figure>

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
| `cut-body` | the first success GET per path ships honest status and headers, then a truncated body and a close. `fake-length` lies the declared `Content-Length` 1024 bytes high | a mid-body break is a retryable transport failure: the retry resumes from the part and the download lands byte-perfect, lie or no lie | core: `cut_body_down_completes_through_the_part`, `fake_length_down_recovers_through_a_short_read` · CLI: `cut_body_down_completes_through_the_part` · mock: `cut_body_first_get_truncated_then_honest`, `cut_body_fake_length_lies_high` |
| `drop-connection` | the first request per path is reset right after its request line and headers are read: the body is never read, nothing is answered or stored | a reset is retryable like any broken transport | core: `drop_connection_is_retried_through_the_client` · CLI: `drop_connection_up_recovers` · mock: `drop_connection_resets_first_request_per_path` |
| `freeze-upload` | the PUT connection is held right after the head: the body is never read and no response is ever written (the hold is bounded, then the connection drops) | the attempt-level stall watchdog aborts a frozen upload instead of hanging | core: `stalled_upload_fails_within_the_stall_window` · CLI: `failing_up_still_flushes_ndjson_events` |
| `sizeless` | success (200) GET and HEAD answers for present objects carry no `Content-Length`. 206, 416 and misses keep it | a 2xx without a length is `Broken`: refused, never treated as `Absent` | core: `sizeless_head_refuses_instead_of_overwriting` · CLI: `sizeless_up_refuses_instead_of_overwriting` |
| `slow` | bodies arrive in small delayed chunks | stall detection aborts the attempt, the retry loop continues, and an honest refusal is the last resort | core: `stalled_download_retries_then_refuses` · CLI: `slow_down_succeeds` · mock: `slow_get_roundtrip` |
| `foreign-marker` | stored markers carry a foreign digest | a remote `Broken` object is never overwritten: the digest check refuses and the part is dropped | core: `down_digest_mismatch_refuses_and_drops_part` · CLI: `foreign_marker_down_refuses` · mock: `foreign_marker_zeroes_stored_digest` |
| `markerless` | markers are acknowledged with `201` but silently dropped | a Markerless remote is re-uploaded by `up`, and `down` writes the marker it computed from the received bytes | core: `markerless_remote_is_re_uploaded`, `down_markerless_remote_writes_computed_marker`, `mirror_completes_a_markerless_source` · CLI: `markerless_down_writes_computed_marker` · mock: `markerless_loses_marker_keeps_bytes` |
| `auth-401` | `401` with a `WWW-Authenticate` challenge without valid Basic credentials | auth failures fail fast with no retries and name the URL | core: `auth_gates_every_request` · CLI: `auth401_exit_codes` · mock: `auth_401_gate` |
| `auth-403` | `403` with a `forbidden` body without valid Basic credentials, on any method | a 403 maps onto the auth error (exit 3) exactly like a 401 | core: `auth_403_surfaces_as_the_auth_error` · CLI: `auth403_exit_3` · mock: `auth_403_gate` |
| `doc-drift` | after `enable_drift`, every GET of `*/version.json` serves a synthesized version document with a ghost artifact | `version.json` is an ordinary name. The legacy fixture is kept as a trap for future special-casing and as a target for external harnesses | CLI: `doc_drift_down_takes_the_drifted_document` · mock: `doc_drift_synthesizes_between_enable_and_disable` |
| `flaky` | the first K requests per path answer `503` | one invocation recovers through retries | core: `single_call_recovers_through_flaky` · CLI: `flaky_up_recovers_in_one_invocation` · mock: `flaky_serves_503_for_first_k_requests` |
| `retry-after-503` | the first request per path answers `503` with a `Retry-After` pause | one invocation honors the server pause and recovers | core: `retry_after_503_pauses_instead_of_backoff` · CLI: `retry_after_503_up_recovers` · mock: `retry_after_503_serves_one_pause_then_atomic` |
| `rate-limit` | the first N requests per path answer `429` with a `Retry-After` header and an empty body, then serve like atomic | the client honors the `Retry-After` pause, spends attempts on the 429s and still lands the transfer in one invocation | core: `rate_limited_up_recovers_in_one_invocation` · CLI: `rate_limit_up_retries_and_succeeds`, `golden_ndjson_retrying_holds` · mock: `rate_limit_serves_429_then_atomic` |
| `redirect` | GET and HEAD answer `301` with `Location` and an empty body. Writes stay atomic | redirects are never followed: a 301 surfaces as the plain HTTP error (exit 3) | core: `redirect_refuses_reads_with_http_301` · CLI: `redirect_exit_3` · mock: `redirect_answers_301_on_reads_only` |
| `readonly` | every DELETE answers `403`, the store keeps everything | the read-only repository refuses `rm` and `point` with exit 1, and a refused run has deleted nothing | core: `readonly_refuses_rm_and_changes_nothing`, `point_clear_on_readonly_refuses` · CLI: `rm_readonly_refuses_and_keeps_bytes` · mock: `readonly_refuses_every_delete` |
| `no-service` | the service REST endpoint answers `404` like a store miss. Storage is `atomic` | `service repos` refuses with exit 3 and the server-root hint. Every storage invariant is unaffected | core: `service_repos_missing_refuses_with_the_root_hint` · CLI: `service_repos_missing_prints_the_root_hint` · mock: the seeded document is simply absent |
| `search-400` | the search API answers `400` when the `repository` parameter names an unknown repository. Served repositories and other requests behave like `atomic` | a repository-scoped search 400 is diagnosed: `ls` refuses with exit 3 and the missing-or-non-raw-repository hint, and a search 404 keeps the generic enumeration hint | core: `ls_unknown_repository_hinted_on_400_not_on_404` · CLI: `ls_unknown_repository_prints_the_repository_hint` · mock: `search_unknown_repo_answers_400_known_repos_pass_through` |

## Client coverage

The scenario matrix above is the server contract.
This one is the client contract: every wire invariant against the implementations that claim it.

| Invariant | `nexus-raw-core` | `nxr` | `@nexus-raw/nxr` | `clients/python` |
|:----------|:----------------:|:-----:|:----------------:|:----------------:|
| Completion is bytes plus a verified marker. A diverged complete object is never overwritten | yes | yes | via core | yes |
| The marker is `sha256sum -c` format and follows the bytes of the same name | yes | yes | via core | yes |
| The marker names the artifact relative to the addressed base | yes | yes | via core | yes |
| A directory URL refuses without an explicit enumeration source | yes | yes | via core | n/a |
| Resume sends `Accept-Encoding: identity`; a complete part finalizes on `416` | yes | yes | via core | identity only |
| A markerless remote is completed by computing the marker from the received bytes | yes | yes | via core | yes |
| A foreign or unparseable marker refuses the name | yes | yes | via core | yes |
| `up` never deletes a remote name | yes | yes | via core | n/a |
| `mirror` refuses a diverged destination and never deletes | yes | yes | via core | n/a |
| `rm` deletes the marker before the bytes, 404 is done, read-only is refused | yes | yes | via core | n/a |
| Channels are ordinary token files. `--if-forward` is dotted-numeric | yes | yes | via core | yes |
| Exit codes 0/1/2/3 and the `hint:` line are stable | yes | yes | via core | yes |
| NDJSON shapes are additive and pinned by fixtures | yes | yes | n/a | n/a |
| Credentials never echo and TLS verifies by default | yes | yes | via core | yes |
| Names are `[A-Za-z0-9._-]` segments and `.sha256` and `.part` are reserved | yes | yes | via core | yes |
| Segmented transfers: bounds, single-stream fallback, per-span resume | yes | yes | via core | n/a |
| The clone fact cache is a hint, never authority | yes | yes | via core | n/a |

`via core` means the binding calls the `Nxr` facade and carries no wire logic of its own.
Its independent surface is checked by `scripts/check-bridge-surface.sh`, and the only additional behavior test is `diff_reports_the_four_states`.
Every other row inherits the core suite.
`n/a` means the surface does not exist in that client, not that it is untested.

The named pins behind the rows:

| Invariant | Pinned by |
|:----------|:----------|
| bytes plus marker, never overwrite a divergence | core: `divergent_complete_refusal_is_wire_covered`, `down_digest_mismatch_refuses_and_drops_part` · CLI: `foreign_marker_down_refuses` · python: `test_foreign_marker` |
| marker format and order | core: `up_generates_markers_by_default` · CLI: `up_generates_markers_by_default`, `put_get_roundtrip_with_sha_sibling` · python: `test_atomic_roundtrip` |
| marker naming | core: `up_generates_markers_by_default` · CLI: `up_marker_names_the_nested_path` · python: `test_atomic_marker_names_the_decoded_segment` |
| enumeration is explicit | core: `rm_without_names_refuses_with_enumeration_error`, `down_missing_name_is_data_error` · CLI: `down_without_enumeration_needs_a_source` |
| identity and `416` | core: `down_resume_of_complete_part_finalizes_without_refetch`, `get_primitive_resumes_with_range` · CLI: `get_resume_with_matching_part_succeeds` · python: `test_atomic_get_sends_identity_encoding` |
| markerless completion | core: `down_markerless_remote_writes_computed_marker` · CLI: `markerless_down_writes_computed_marker` · python: `test_markerless` |
| foreign marker refusal | core: `down_digest_mismatch_refuses_and_drops_part` · CLI: `foreign_marker_down_refuses` · python: `test_foreign_marker` |
| `up` never deletes | the protocol table and the `classify` unit path: a remote-only name is `mismatch`, never a delete |
| `mirror` never deletes | core: `mirror_refuses_diverged_destination_untouched` · CLI: `mirror_refuses_a_diverged_destination` |
| `rm` order and idempotency | core: `rm_removes_a_whole_version`, `second_rm_is_all_404_and_exits_clean`, `readonly_refuses_rm_and_changes_nothing` · CLI: `rm_removes_whole_version_and_reruns_clean`, `rm_readonly_refuses_and_keeps_bytes` |
| channels and dotted order | core: `channel_set_get_and_forward_guard` · CLI: `channel_set_get_and_if_forward` · python: `test_atomic_channel`, `test_channel_set_refuses_multiline_tokens` |
| exit codes and hints | core: `auth_403_surfaces_as_the_auth_error`, `redirect_refuses_reads_with_http_301` · CLI: `auth401_exit_codes`, `dead_base_exit_3`, `unsafe_name_is_misuse_exit_2` · python: `test_malformed_urls_are_misuse` |
| NDJSON shapes | CLI: `golden_ndjson_up_down_hold`, `golden_ndjson_rm_holds`, `golden_ndjson_error_holds` |
| credentials and TLS | core: `auth_gates_every_request` · CLI: `doctor_probe_rejects_rejected_credentials`, `doctor_exit_codes` · python: `test_auth_401`, `test_auth_403` |
| name grammar | core: `scan_rejects_unsafe_names` · CLI: `unsafe_name_is_misuse_exit_2` · python: `test_put_refuses_unsafe_names` |
| segmented transfers | core: `segmented_down_lands_the_same_bytes_as_one_stream`, `segmented_down_resumes_finished_spans`, `segmented_down_falls_back_when_range_is_ignored` · CLI: `down_segments_matches_the_single_stream`, `get_segments_matches_the_single_stream` |
| clone cache | core: `clone_second_run_skips_and_writes_the_cache`, `clone_corrupt_cache_falls_back_to_hashing`, `clone_refuses_a_diverged_local_object` · CLI: `clone_copies_a_repository_and_skips_on_the_second_run` |

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
To trace a failure from the symptom a consumer sees, start with [when it breaks](../how-to/troubleshoot.md). The error taxonomy is in [errors and exit codes](../reference/errors.md).
