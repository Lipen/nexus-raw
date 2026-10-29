# Changelog

All notable changes to the `nxr` CLI, the `nexus-raw-core` library, the `nexus-raw-tui` browser and the `mock-nexus` server are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- A `static-host` mock scenario pins the bare file-server profile: reads and ranges work, every write is the read-only refusal (405, exit 1), and the whole service plane answers 404, the search API included.
  The core, the CLI and the Python client test it, so the read profile off Nexus stops being a manual nginx probe.
- A `warning` event, rendered as a `warning:` line on stderr and as `{"event":"warning","text":...}` under `--json`, names the rows an enumeration left out because they are outside the artifact grammar: `down --ls`, `clone`, `diff --via search` and the list command share it, so a transfer no longer loses such a name silently.
  The Node bindings surface the same event, and the skill carries the exit-code rule.

### Security

- `nxr-tui` downloads keep every server-chosen name inside the destination.
  A repository row survives only on the session origin and with a name that is one safe directory segment, so `--smoke` no longer deletes `{root}/{hostile name}` and a whole-repository download no longer lands in an absolute path.
  A listing name that fails the artifact grammar (a percent-encoded `/` decodes into one) is refused before a transfer instead of being joined onto the destination.
- The TUI origin check parses both URLs through the core's `normalize_base`, so a repository URL carrying userinfo, a query or a fragment shares no origin, and a refresh filters exactly like the first load.
  The status line, the error modal and the smoke output print `scheme://host[:port]`, never a URL with a password in it.

### Fixed

- A single-file download in `nxr-tui` writes the `.sha256` marker it computed, so a tree the browser filled passes `nxr verify` offline, exactly like a `down` tree.
  The marker of the previous download is dropped before the next transfer: left in place it became the resume guard's expectation and refused a legitimately changed object.
- The browser's single-file download refuses a symlinked directory on the way to the target, the same rule the directory transfer applies, and it accepts a folder name with a space or unicode while still refusing a segment with a decoded `/`, `..` or a control byte.
- A repository name ending in a dot or a space, or made only of them, is refused by the browser's name rule: on Windows such a name resolves to a different directory.
  The save-as dialog applies the same rule to a typed folder name.
  The marker write is one core helper (`sibling::write_marker`) now, shared with `down` and the clone seal.
- A locally broken bytes/marker pair names its repair: the error says to delete the stale sibling, which the next scan regenerates, or to restore the bytes. The old hint pointed at "two sides" for a disagreement that is purely local.
- `clone` creates a missing destination the way `down` does, instead of failing with an I/O error named after a part file under a directory that does not exist.
- `nxr-mount` refuses a URL outside `/repository/<name>/…` at startup with exit 2 and the listing's own message, instead of mounting a tree whose listings can never resolve.
  The check runs before any request, so a wrong URL costs nothing.
- The `--ls` and `ls --assets` URL-shape refusal names the alternative: a host without the search API takes `--name` or `--names-from`, and the message says so instead of leaving the user with a Nexus path rule.
- The Node bindings refuse what the CLI refuses, instead of picking silently:
  `ls` together with `names` is misuse (it used to drop the names, so `rm` with both deleted the whole listing), `up` with a prefix selection that holds no artifacts is misuse (it used to resolve an empty summary), and `get` with `cont` and no `out` is misuse (it used to ignore the resume).
  `sha` on an unreadable local file is an I/O refusal (exit 1) like the CLI, not misuse, and `clone` rejects `state` next to `noState` instead of letting one win.
- A `diff` with `prefixes` filters the whole report in the core, so a local-only name outside the prefixes no longer reports as `missing-remote`.
  The CLI and the Node bindings share the rule, and the CLI's own filter is gone.
- A byte body staged by the Node `put` carries the artifact's own name, so its progress events name the upload instead of a temp file.
- The Node result types mark an unknown fact as optional (`sha256?`, `contentType?`, `size?`, `from?`, `digest?`, the delta sides): the bridge omits a `None` instead of sending `null`, and the delta-side docs claimed `null` before.
  The Node `diff` examples in the README and the quickstart pass the enumeration source the call needs.
- The Python reference client classifies a 405 to a PUT as `ReadOnlyError` (exit 1, the hint names the group repository), matching the core, and exports the class in `__all__` like every other documented error.
- `ls --assets` names the names it leaves out: a server can hold a row outside the artifact grammar (a space, a percent-decoded slash), which the protocol cannot transfer, so the listing reports the count instead of dropping it silently.
  The `hidden` NDJSON event now carries a `reason` (`marker` from the tree listing, `grammar` from `--assets`).
- The mount leaves a listing name that is not one path component (a decoded `/`, a nul byte, a dot segment) out of `readdir`: `lookup` refuses such a name, so the row could never open.
- The HEAD that opens a segmented `down` or `get` is a transfer request: a 5xx or 429 on it retries (and honors `Retry-After`), and a 401/403 takes the auth verdict, exactly like the range GETs that follow.
  `nxr head` keeps reporting any status: its HEAD is a result, not a failure.
- A `405` to a PUT is the read-only refusal, the class a `DELETE` already had: exit `1`, never retried, and the hint names the group repository instead of asking the user to retry or to check their credentials.
  A group repository refuses every write by design, and a host without PUT support answers the same way.
  `Error::ReadOnly` stays the same variant, its hint is now method-neutral and names the host case too.

## [0.24.1] - 2026-10-29

### Security

- The compression stack moves to `zstd` 0.14.3, past [RUSTSEC-2026-0391](https://rustsec.org/advisories/RUSTSEC-2026-0391).
  The vulnerable `with_context` constructors ranged over `>=0.13.2, <0.14.3`, and `zstd` arrives through `reqwest` for HTTP body decoding.

### Fixed

- The `-q/--quiet` help text, the output reference and the generated shell completions agree that failures survive quiet mode.

## [0.24.0] - 2026-10-24

### Added

- `--ca-bundle FILE` (also `NXR_CA_BUNDLE`) adds a PEM bundle of trust roots for a private CA.
  It is native only and conflicts with `--tls-insecure`.
- `get --range START-END|START-` reads one byte span, to a file or stdout.
- `up --prefix PREFIX` publishes only the names under a whole-segment prefix, keeping the relative path.
- `mirror --plan` is the `--dry-run` alias.
- `nxr-tui --ca-bundle FILE` (also `NXR_CA_BUNDLE`) and `--tls-insecure` apply one TLS policy to the session.
- `nxr-tui --no-color` and a non-empty `NO_COLOR` drop the two semantic colours.
- The Node bindings accept `caBundle` for a private CA and `prefixes` on `up`.
- The Node `get` accepts `range`, with the same span rules as the CLI.
- The Linux release archives carry `nxr-mount` next to `nxr`, and the Homebrew formula installs it when present.
- `Nxr::scan_prefixes` and `Nxr::get_range` expose the scoped scan and the one-span read to library consumers.
- `Nxr::sibling_digest` reads a `.sha256` marker without a transfer. A malformed marker is `Mismatch`.

### Changed

- The repository's terminal output follows one model: results on stdout, failures, warnings and hints on stderr, and colour is decided per stream.
  Hints carry commands in backticks, `check-docs` and the real-Nexus battery stay quiet on success, and the battery keeps a TTY heartbeat without a hardcoded total.
  The demo reports its mock scenario count and aligns its `--` comments.
- The local scan refuses a symlink or any other non-regular entry with exit 2 and a hint, instead of silently dropping it from the upload.

### Fixed

- The mount evicts cached blocks per inode and counts the released bytes exactly, so the cache budget no longer drifts.
- The mount help and reference state the minimum `--block-kib` and `--cache-mib` values.
- The `.part` suffix joins the reserved names, and a local `.part` staging file is an explicit misuse in `up` instead of a silent omission.
- `get --range` refuses a start past the end and replaces a symlinked output instead of following it.
- The `UnsafeName` hint names the regular-file rule, so a symlink refusal no longer points only at the name grammar.
- The help hides the `NXR_CA_BUNDLE` value instead of echoing the path.
- `up --prefix` scans only the selected subtrees and validates explicit names path by path, so an unselected symlink no longer refuses the run and the tree is not walked twice.
- The scan refuses a symlinked source root for plain `up` and `diff`, matching the scoped prefix scan.
- The mount percent-encodes each path segment, so foreign names with spaces, `?`, `#`, `%` or unicode resolve correctly.
- The mount forgets a node when the kernel drops its last lookup and reports synthetic inodes for unvisited listing entries, so long browsing sessions stay bounded.
- The wasm32 build compiles again: the native prefix helpers are gated, and the gate checks the core for the sandbox target.
- The unreleased `--cache-dir` mount experiment was removed before it shipped. The mount stays memory-only.

## [0.23.0] - 2026-10-22

### Added

- The Homebrew formula, `Formula/nexus-raw.rb`, with this repository as its tap (`brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw`).
  `just brew-formula` regenerates it from a release's `SHA256SUMS`, and `scripts/brew-formula.sh --check` verifies it.
- `nxr-mount` (Linux): a read-only FUSE view of a raw repository.
  Listings go through the search API, reads are lazy ranged GETs over a bounded block cache, and `.sha256` markers stay hidden.
  Every write answers `EROFS`.
  The binary is a separate crate, so `nxr` keeps no FUSE dependency and stays portable.
- `Nxr::read_range`, a byte span into memory that skips to the span without buffering the whole object when the server ignores `Range`, and `Nxr::ls_at`, a listing of any directory URL: the two core paths the mount is built on.
- `NexusClient::get_range`, the transport primitive behind `read_range`.
- `nxr down <file-url> <dst-dir>`, and the Node `down` with no `names` and no `ls`, download that one artifact with the same marker, resume and digest checks as a directory sync.
  The parent directory is the base and the leaf is the name, and an explicit `--name`, `--names-from` or `--ls` keeps the URL as the directory to enumerate.
- The wasm sandbox lists and uploads.
  `nxr_put` PUTs the bytes and the `.sha256` marker, the proxy forwards PUT with a request-body cap, and both the Node fake and the ServiceWorker fake store it.
  `Nxr::put_bytes` is part of the wasm surface: it is an in-memory POST-free path, not a file operation.

### Fixed

- The composite action installs the release named by its tag when `version` is empty, instead of always the latest release.
- The mock server refuses an inverted `Range` and an oversized `Content-Length` or chunked size instead of panicking, honors `spec.redirect`, and lists each repository once in the scoped collection.
- The mirror staging lock is held by the creating run, a pre-created symlink at the staging path is replaced instead of followed, and the segmented resume is gated on the object digest.
- An artifact segment with the reserved `.nxr` prefix is refused, so local part, segment and cache files can never collide with a repository object.
- `put_bytes` validates the marker name before the bytes are PUT, and a zero-length `read_range` still reports an absent object as `None`.
- `up --wait-index` matches a grouped or repository-root base, `service eula --accept` propagates a 401/403 instead of reporting no gate, and `service status` reads a refused probe as `false`.
- `diff --prefix` filters local-only entries too, and `up --if-changed --plan` uses the if-changed classification.
- `doctor` falls through to the repository HEAD on a 404 status and builds the deep service probes at the origin, and `service assets` no longer slices a multibyte checksum.
- `mirror` and `mv` fall back to the shared `-u`, `--ls` cannot be combined with `--name` or `--names-from`, and a clap parse error still carries a `hint:` line and the NDJSON error object.
- The alias URL expansion no longer uses raw pointers, and a broken TOML line no longer echoes an inline password.
- Server-supplied text is escaped in human output, credential-bearing base URLs are no longer echoed, and the 401/403 body read is bounded.
- The TUI drops a repository URL whose origin differs from the configured server.
- The PyPI publish waits for the gate, its action is pinned to a commit, and a new gate rejects any action pinned to a tag.
- The panel and the wasm sandbox bound their targets to configured origins, validate upload names before writing, queue uploads, and ignore stale listing responses.
- The wasm server answers 413 before closing an over-cap body, the page unregisters a stale worker, and the node example kills its mock on exit.

## [0.22.1] - 2026-10-21

### Fixed

- The shipped `nxr` skill teaches `clone` and `--segments`, lists every command that reads the search index, and includes `mv` in the enumeration contract.

## [0.22.0] - 2026-10-20

### Added

- `down --segments N` splits each artifact into `N` concurrent closed Range spans, 2..=64.
  A finished span survives an interrupted run, so a rerun fetches only the unfinished spans.
  The spans assemble into the usual part file, one pass hashes it, and the marker check decides the rename.
  The worker budget still caps the total concurrency, and a zero-byte object, an unsplittable one or a server that ignores Range falls back to the single-stream path.
  The core exposes the same transfer as `Nxr::down_with_segments`, and the Node bindings accept `segments` on `NxrDownOpts`.
- `nxr clone` copies a whole repository into a local directory: every artifact and marker is verified, an interrupted run resumes, and a local object whose bytes match its remote marker but whose local marker is gone is sealed without fetching the bytes again.
  An optional local fact cache (`<DIR>/.nxr-state.json`, `--state`, `--no-state`, `--verify`) skips re-hashing files whose size, mtime and marker digest still agree.
  A missing, corrupt or foreign cache falls back to hashing.
  The summary gains the additive `sealed` count, and the Node bindings expose `clone()` and `clonePlan()`.

## [0.21.0] - 2026-10-19

### Added

- The transfer `summary` gains `failures`: the same artifacts as `failed`, as `{"name","code"}` objects whose code is the exit class (1 data, 2 misuse, 3 transport).
  `failed` keeps its legacy strings (a mismatch still carries `name: detail`), so the new array is additive for consumers.
  The Node bindings mirror the array on `NxrSummary`.
- `Nxr::audit_filtered` and `sync::audit::AuditFilter`: an audit narrowed by exact names and whole-segment prefixes before any request.
- The `retry-after-503` mock scenario: the first request per path answers `503` with a `Retry-After` pause, so the 5xx pause is pinned without changing the plain-503 `flaky` contract.

### Changed

- The `put` source type is `Uint8Array` instead of `Buffer`.
  A Node `Buffer` is a `Uint8Array`, so existing callers stay compatible, and the shipped declarations no longer need `@types/node`.

### Fixed

- The composite action installs `nxr` again: it resolves the release from `github.action_repository` (the caller's repository was never the action's) and verifies the archive against the published `SHA256SUMS` instead of a per-archive `.sha256` asset that the release never uploaded.
  The checksum check accepts the Windows binary-mode marker and falls back to `shasum` where GNU coreutils is absent.
  A CI job runs the action end to end on Linux, Windows and macOS.
- `get --continue` keeps the partial file when a transfer fails, so the rerun the hint recommends resumes instead of starting from zero.
- A resumed download refuses a symlinked or concurrently changed part, validates the `Content-Range` end and the bytes on disk, and restarts a stale part inside the same attempt, so `--retry 1` still self-heals.
- A `down` whose rename or destination check fails keeps the verified part for the rerun.
- `Retry-After` is honored on every retryable status, not only `429`: a restarting Nexus that answers `503 Retry-After: 30` is waited out instead of retried after half a second.
- `get --segments` accepts 2 to 64 spans, runs them under the same worker ceiling as every other transfer, and completes an empty object instead of dividing by zero.
  A segment count outside the bounds is misuse (exit 2).
- The uploader re-checks the source size per attempt and reads exactly the declared bytes, so a size change between the plan and the send is an I/O error instead of a body that disagrees with `Content-Length`.
- `scan` rewrites backslashes only on Windows: on Unix a backslash is a filename byte and the grammar rejects it instead of uploading a phantom path.
- `audit --prefix` obeys the same whole-segment grammar as every transfer filter, runs before marker and byte requests, and `audited` counts the objects the tree held rather than repeating the finding count.
  `facts_available` reports whether the index answered, not whether the filter selected anything.
- `mv --json` writes a pure NDJSON stream: the plan phase headers are human output.
- `sha` expands `-R` aliases for relative URLs, keeps an existing local path local, and reports a missing file or a non-http scheme as misuse (exit 2).
- The live progress counter counts skipped files in its total.
- `doctor --deep` without a URL is refused as misuse instead of silently skipping every server probe and reporting "all checks passed".
- The release gate runs the full `just check` set (prek, cargo-deny, Python conformance), CI runs the prek hooks, the docs workflow rebuilds on the files its checkers read, and zensical is pinned.
- The Rust example README teaches the current minor and joins the version gate.

## [0.20.0] - 2026-10-18

### Added

- `nxr` ships a container image: `ghcr.io/lipen/nexus-raw:<version>` is the static release binary on a distroless base that carries the trust store TLS verification reads, runs as `nonroot`, and is published for `linux/amd64` and `linux/arm64` by the release workflow, from the archives that release already built.
  A `COPY --from=ghcr.io/lipen/nexus-raw:<version> /nxr /usr/local/bin/nxr` puts it in a downstream image.

### Fixed

- Enumeration survives a repository bigger than 5000 objects: the search walk was capped at 100 pages of 50 rows, so `ls`, `down --ls`, `diff --via search` and `audit` refused with `cannot enumerate` past that size.
  The bound is now per walk at 1000 pages, and a directory listing asks the search API with `q=<directory>/`, which cuts the pages down to that directory (13 to 61 pages instead of 122 on the measured 6095-object repository) while the client-side whole-segment filter stays the answer.
  The property that a directory query never drops a name is versioned, not verified at run time: the stand battery pins it against the pinned Nexus image.

## [0.19.1] - 2026-10-18

### Fixed

- The npm platform packages ship an executable `nxr`: the artifact hop between the addon job and the publish job flattens file modes, so the staged binary arrived without its exec bit and the published package could not spawn it.
  The 0.19.0 platform packages are inert orphans (the main package never shipped at that version), and the publish job now restores the bit and refuses to pack a binary without it.

## [0.19.0] - 2026-10-18

### Added

- `npm install @nexus-raw/nxr` brings the CLI with it: every platform package carries the `nxr` binary next to the addon, and the bin wrapper spawns it, so a node project needs no separate download.
- `nxr` ships an Agent Skill: `skills/nxr/` holds the command map, the machine contract, the recipes and the server quirks that are easy to get wrong.
  `scripts/install-skill.sh` drops it into an agent's skill directory, and the how-to explains the format.
- Every `summary` NDJSON line names its stream: a transfer carries `"kind":"transfer"`, the audit summary `"kind":"audit"`, and the napi event type declares it, so one stream is parseable without knowing which command wrote it.
- The `--json` records pinned by goldens are a contract: their shapes hold, new fields are additive, and a consumer ignores unknown fields and key order.
  `error`, `rm` and `audit` join the pinned set.
- The bench ecosystem gains its report pipeline: `just bench-report <tag>` runs the suite and writes `bench/reports/<tag>.json` plus a rendered `.md`.
  The record carries the ladders the run actually covered, the machine and the commit.
  `just bench-compare <a> <b>` prints the delta with each side's spread and warns when two records are not comparable.

### Fixed

- `nxr doctor` names a rejected probe as an auth failure: a 401 or 403 on the probe now points at the credentials instead of the network.
- The Python reference client refuses a URL without a host as misuse (exit 2) instead of letting it fall through to a transport error.
- `mock-nexus` seeds the search download URLs with the repository root slash they need, so a conformance suite driving the service API sees resolvable URLs.

## [0.18.0] - 2026-10-16

### Added

- `put --stdin`: the body streams from stdin as one PUT (`printf … | nxr put URL --stdin [--sha]`).
  The pipe is buffered whole (Content-Length is part of the protocol), the digest rides the same buffer.
  `-f` and `--stdin` are mutually exclusive, and calling with both (or neither) is refused as a misuse.
- `Nxr::put_bytes(url, body, sha)`: one-call PUT of an in-memory body, the library surface under `--stdin`.
- `nxr doctor --json` and the human report carry a remediation on every failed check:
  `next` names the command or configuration to try next (credentials for 401/403, the base URL and
  `--deep` for a transport refusal, the EULA accept for the 403 write gate).
- The Python reference client ships on PyPI as `nexus-raw`: `pip install nexus-raw`, the same
  stdlib-only `nxr.py` the conformance suite drives, trusted publishing, no tokens.
- The unscoped npm name returns as a bridge: `nexus-raw` re-exports `@nexus-raw/nxr` 1:1
  (babel-core's bridge pattern), version in lockstep, so `require('nexus-raw')` keeps resolving
  to current code. Versions below 0.17.0 are deprecated with the rename message.
- The release carries a `llms.txt` snapshot: site URLs rewritten to the raw markdown at the tag,
  so an agent pinned to a version reads that version's documentation.

### Changed

- `cargo deny check` joins `just check` and CI: RustSec vulnerabilities and yanked crates deny,
  licenses allowlist, sources policy. rustls moved to 0.23.46 (four advisories closed by upgrade).
- The docs gain a how-to: comparing two servers through `mirror --dry-run`, the plan is the delta.

## [0.17.0] - 2026-10-15

### Changed

- `up --wait-index` polls the search index with exponential backoff instead of a fixed interval: 500ms, then 1s, 2s, 4s, capped at 5s.
  A 30s budget costs roughly 8 listing requests instead of roughly 60.

### Removed

- `nxr point --clear <URL>`: the `--clear` flag is gone, `nxr point <URL>` deletes directly.
  The flag-guarded misuse case never had a success path, so the guarded form was a dead branch of the interface.
- Documentation advertised manifest-based enumeration that was removed in 0.12.0: the `AGENTS.md` down-source list, the core README example and the CLI README quick start now show the real sources (`--name`, `--names-from`, `--ls`).
  The rust example and its tutorial were ported to explicit enumeration, and the example compiles again against the published crate.
- `nxr clean`: the command is gone, enumerate and delete with `nxr rm` itself.
  The command guessed versions by path-segment grammar, and a guessed scope is the one thing a destructive command must never do: a channel file named `stable` once passed as a "version".
  Retention is a policy, and policies live on the user side: [the keep-N recipe](https://lipen.github.io/nexus-raw/how-to/retention/) builds it from `ls`, `channel get` and `rm` with explicit enumeration.
  The library surface loses `Nxr::clean_plan`, `Nxr::clean_execute`, `Nxr::ls_versions`, `search_versions` and `validate_version` with it, and the npm package loses `lsVersions`.
- `rm --dry-run` and `mv --dry-run`: the flags are gone, both commands plan by default and delete only with `--yes`.
  Deletion being the default unless flagged was the dialect inversion: a destructive command must demand the explicit go.
  The npm `rm` surface follows: the promise resolves to the plan by default, `confirm: true` deletes, and `dryRun` is gone from `NxrRmOpts`.
- The implicit `version.json` claim in `mirror` and `mv`: no name is special, the transfer order follows the user's enumeration verbatim.
  The explicit `up --claim-first` is untouched: an opt-in claim stays a user decision.

## [0.16.0] - 2026-10-15

### Added

- `nxr clean <BASE_URL> [--keep N] [--yes]`: retention over whole versions.
  The plan names every version outside the newest `--keep` (dotted-numeric order) with the count of its names.
  A plan is the default and deletes nothing.
  `--yes` deletes the doomed versions through the regular `rm` machinery: markers before bytes, per-version atomic.
  Channel refs and names outside the recognized versions never enter the plan.
  Requires the search API (like `audit`'s cross-check): a search-dead server refuses the run.
- `up --wait-index [--index-timeout SECS]`: after the run, poll the search index until every uploaded name is visible.
  A real server's index lags behind writes by seconds, so a consumer reading through search right after `up` sees a stale world.
  The wait reports `search visibility: N/M` and degrades to a warning when the budget (default 30s) expires: the upload itself has succeeded.
- `doctor --deep` gains the `eula` probe: `GET /service/rest/v1/system/eula`, strictly read-only.
  `accepted: false` is a warning naming the write gate and the accepting command, a 404 is a skip (no gate on this server), a refusal is a skip (admin rights).
- The mock scenario `search-lag`: the search index serves the empty page for 6s after the first PUT, then a store-backed listing.
  The python reference client skip-lists it: its uploads are synchronous by construction.

### Changed

- The docker/CI images and the docs site ship a composite GitHub Action in this repository: `uses: <owner>/nexus-raw@v…` with `command`, `args`, `version` and `auth` (a secret, passed through the environment, never argv).
- `cargo binstall nexus-raw` resolves the prebuilt release archives through the crate manifest metadata (present since 0.15.0, now documented).

## [0.15.0] - 2026-10-14

### Added

- `nxr audit <REPO_URL>`: the repository integrity report in one call.
  The structural mode reads metadata only: the raw tree (markers included), one small GET per listed marker, and the search facts as the independent cross-check.
  It classifies `orphan` (bytes without marker), `dangling_marker` (marker without bytes), `unparseable_marker` (a body outside the `sha256sum -c` format) and `diverged_fact` (marker digest vs search fact).
  Findings are data, so a run with findings exits 1.
  A fresh upload missing from the lagging index is never a finding: the tree and the markers are the truth.
  `--bytes` additionally downloads and hashes every asset against its marker: the only pass that sees silent bitrot, at the price of reading the repository.
  `--sample N` byte-checks a deterministic subset instead (requires `--bytes`, misuse otherwise).
  Read-only by contract: no audit request mutates the repository.
- `nxr doctor --deep` (with `--probe-name NAME`): the server-behavior probes behind the invocation checks, all read-only.
  `range` (a `bytes=0-0` GET: `206` honored, `200` names a range-stripping proxy that degrades `--segments`), `content-length` presence, `search` liveness, `repo-type` through the service REST API (group members in order), and the server `version` as a diagnostic.
  A probe without a target (an empty repository is a normal case before the first `up`) reports `skipped`, never `ok` or `fail`.
  Warns are advice and keep the exit at 0.
  Probes are independent, one refusing never stops the others.
  Write rights are deliberately not probed: the probe would itself be a mutation.
- The `audit-litter` mock scenario seeds the structural finding classes plus a clean control pair.
  The conformance suite pins the two-mode contract (the structural pass sees no byte corruption, the byte pass verifies the clean pair).

## [0.14.0] - 2026-10-13

### Added

- `nxr get --segments N`: fetch a file through `N` concurrent byte-range requests, assembled positionally and verified against the `.sha256` sibling before the atomic rename.
  A server that answers a ranged GET with the whole body degrades the download to the single-stream path (logged through the events channel, never an error), and an object smaller than the span count degrades to smaller segments.
  `--segments` requires `-o FILE`: a stdout target is a misuse (exit 2), and stdout streaming stays single-stream.
  The wire contract is unchanged: segmented reads are a client-side strategy over `Range: bytes=start-end`.
- Windows joins as a first-class platform: the `mirror` staging lock works there (the lock file is opened with sharing disabled, so a concurrent run for the same pair refuses exactly as on unix), and CI builds and tests the workspace on `windows-latest` next to the ubuntu leg.
- `nxr diff --via search --ls`: the remote side of the delta report is built from the search API's per-asset facts (name, digest, size) in one paged listing, instead of a HEAD/sibling probe pair per name.
  `--prefix` filters the facts client-side. The report shape and exit codes are the plain `diff` ones.
  A searched asset without a search-reported digest counts as changed, never equal.
  A search refusal (400/404) refuses with the `cannot enumerate` hint naming the search API, or with `--fallback` degrades explicitly: a warning line, then the probe-based report over `--ls` or explicit names.
- Adaptive transfer workers (AIMD): a directory transfer starts with `--workers` and halves its in-flight capacity whenever a request fails with a `Retry-After` (a 429), reclaiming permits still held as their transfers finish.
  On a streak of successes the limit recovers one worker per step, never above the configured maximum.
  A lone success after a 429 does not snap the limit back: the recovery slope must fill first.
  The floor is 1 worker and the wire behavior is unchanged.
- `nxr up --if-changed`: names whose remote marker digest equals the local copy skip without a single PUT, changed or markerless names re-upload.
  A skip demands evidence: a missing or foreign remote marker never counts as equal.
  Diverging complete copies re-upload instead of refusing, every other guard (broken local or remote objects, `Missing`) refuses exactly as the plain `up`.

## [0.13.0] - 2026-10-12

### Added

- `--names-from <FILE>` on `down`, `diff`, `rm`, `mirror` and `mv`: read the name list from a file, one name per line, `-` for stdin.
  Repeatable, combines with repeatable `--name` in flag order.
  A line that fails the name grammar is a misuse naming the file and the line number.
  For long lists that would bump into the argv limit: the wire protocol is unchanged, the file names the same artifacts `--name` would.
  The napi surface stays as it is: JS callers pass `names` directly and have no argv limit to work around.

## [0.12.0] - 2026-10-12

### Removed

- **Breaking**: the manifest convention is gone.
  `down`, `diff`, `rm`, `mirror`, `mv`, `up` and `verify` lose the `--manifest` flag, and nothing reads a `manifest.json` at the directory URL anymore.
  Enumeration is explicit: repeatable `--name`, or `--ls` where the server has the search API.
  Without a source the run refuses with `cannot enumerate` and a hint.
  A `manifest.json` on the server is now an ordinary artifact: `nxr` never fetches or parses it.
  The napi options lose `manifest` on the same calls, and the typings drop the conventional-manifest wording: `@nexus-raw/nxr` consumers enumerate with `names` or `ls`.
  A pipeline that needs a machine-readable list publishes one and reads it itself.
  Rationale: a reserved name with a reserved format contradicted the design rule that no name carries protocol meaning (the rule that removed reserved channel pointers in 0.3), and a list the client cannot authenticate is trust the protocol never justified.

### Changed

- The small-object cap refusal and the unparseable server body are transport facts, not misuses: the new `SmallObjectCap` and `ServerBody` error variants exit 3 with their own hints, where `Misuse` (exit 2, check the command line) blamed the user for a hostile or broken server.
  The channel `--if-forward` still repairs a garbage token: the repair arm follows the new class.
  The up command refuses to publish a marker over bytes it does not describe: a file rewritten during the upload is a Mismatch, not a silent divergence behind exit 0.
  A stdout write failure (`nxr get URL | head`) is an I/O condition, not a misuse.

### Fixed

- The TUI drops the cached expansion state on a climb to a parent level: a same-named folder there rendered pre-expanded with the wrong directory's children.
  Presets resolving to an already gathered URL now deduplicate into one tab instead of a second view of the same path.
- The Python reference client follows the same wire contracts: the small-object cap reads with a budget like the Rust transport, and the exit-code map matches the new error classes.

## [0.11.1] - 2026-10-11

### Fixed

- The npm platform addons ship under their generated names (`@nexus-raw/nxr-<platform>`, 5 targets): the 0.11.0 loader could not resolve the short-named addons it was published with, which made `npm install @nexus-raw/nxr@0.11.0` dead on arrival.
  The 0.11.0 npm release is written off.
  The crate and CLI surfaces are unaffected.

## [0.11.0] - 2026-10-10

### Changed

- The npm package is renamed: `nexus-raw` becomes `@nexus-raw/nxr`, and the platform addons become `@nexus-raw/<platform>` (linux-x64-gnu, linux-arm64-gnu, darwin-x64, darwin-arm64, win32-x64-msvc).
  The publish rides GitHub OIDC trusted publishing, no tokens.
  The Windows npm addon joins the matrix: `npm install @nexus-raw/nxr` works on Windows x64 without a local toolchain.
  The old unscoped name will be deprecated after this release settles. The import and the install line change with the name.
- The napi surface grows `serviceAssets()`: the full repository snapshot with server-reported sha256 per asset, the cheapest integrity probe in one call.

### Fixed

- `get --continue` validates the resumed download against the `.sha256` sibling: a garbage or foreign `.part` can no longer silently corrupt the target.
- `ls` names the count of markers its enumerator hides (`N marker objects hidden`), so a short listing is a fact and not a silent surprise.

## [0.10.0] - 2026-10-10

### Fixed

- `get --continue` validates the finished file against the `.sha256` sibling (the local one, or the server's when the local is absent) and refuses with exit 1 on divergence: a garbage or foreign `.part` can no longer silently corrupt the target.
  The corrupted part is removed, the target path stays untouched.
- `doctor` probes `/service/rest/v1/status` instead of a HEAD of the repository root, and the probe line explains that any non-5xx answer means the server is reachable.
- A storage 404 (a missing object or version) exits 1 as a data fact, not 3 as transport: scripts branch on 0 proceed, 1 absent, 3 retry.
  The service-API 404 keeps exit 3.
  The docs had promised the data class since the agent cookbook landed, the code disagreed, an external agent caught the divergence.
- Deep listings (`ls`, `lsAssets`) no longer send the `group` search parameter: a real Nexus matches it as Maven coordinates, not raw path prefixes, which silently emptied deep listings against a live server.
  The scoping is the client-side prefix filter, as the shallow listings already did.

### Added

- Named remotes: `nxr -R <alias>` reads `~/.config/nxr/config.toml` (or `$NXR_CONFIG`) and expands the URL arguments that look relative onto the alias URL, plus supplies the credentials (env variable names, inline, or a `pass_cmd`).
  The file is read only when the flag names an alias: without `-R` the call keeps the curl-model contract.
  `nxr complete` knows the flag, `doctor` accepts an alias-expanded URL.
- `nxr --help` ends with agent entry points: the cookbook URL, the doc index and the source, plus the one-line contract (hint on every error, NDJSON via `--json`, the exit-code taxonomy).

## [0.9.0] - 2026-10-10

### Added

- The service overview over the REST API: `nxr service status` for liveness and writability, `nxr service repo` for the repository behind a URL (longest-prefix match, admin detail), `nxr service assets` for content listing over the paginated search (sha256, continuation tokens beyond 1000 entries, `--prefix` and `--q` filters), and `nxr service eula` to read and accept the CE 3.79+ EULA gate (`--accept` is idempotent).
- Errors carry `server says:` in the hint: the short response body of a 4xx/5xx, so an agent sees the server's own words (the EULA 403 hint names the accepting command).
- The mock server grows 13 service scenarios: status empty or versioned or down, trimmed and scoped repository collections, the admin-only repository detail, prefix matching, asset pagination and filters, the EULA gate and its absence, the detail-in-hint behavior.
  The conformance table is the contract: core and CLI suites drive every scenario.
- The agent-facing docs surface: the one-page cookbook (intent to command to expected result, 17 verbs) under how-to/agents, the machine-readable `llms.txt` index shipped with the docs site in the llmstxt structure, and a `scripts/llms-check.sh` gate wired into `just check-docs` and the docs workflow.
- The Python client honors `Retry-After` on 429 with a dedicated scenario.

### Changed

- The real-Nexus stand moves to `NEXUS_PORT` (default 18081), so it never fights a locally running Nexus, and a `run.sh` orchestrator owns the lifecycle: boot, bootstrap with narrated waiting, the battery, cleanup on any exit, group-interrupt verified.

## [0.8.0] - 2026-10-08

### Added

- Linux aarch64 joins the release: a native `ubuntu-24.04-arm` leg builds the static CLI archive `nxr-linux-aarch64.tar.gz` and the `nexus-raw-linux-arm64-gnu` npm addon, and every platform list in the docs and the READMEs follows.
- The npm staging marks the gnu linux addons with `libc: [glibc]`, so npm on Alpine (musl) skips them instead of installing and failing at `require`.
- Role quickstarts under Tutorials: a Rust page on the `Nxr` facade, a Node page on the npm bindings and a terminal page of curl-style one-liners, every command verified against a running mock.
- The bench ecosystem in `bench/`: a standalone workspace with criterion benches for scan, diff, up and down against `mock-nexus`, a deterministic tree generator and `just bench` for manual runs, with the baseline-and-report workflow in `bench/README.md`.
  Fully opt-in: nothing bench-shaped runs in CI or the gate.
- The Python reference client in `clients/python`: the protocol on the standard library alone (get/put/head/sha with digest verification, ls over the paginated search, channels, retries honoring `Retry-After` on 429), with a conformance suite that reads the mock scenario list as the contract: every scenario is tested or explicitly skip-listed, and an unclassified scenario fails the suite.

### Fixed

- The mock server rides out transient `accept()` errors (connection aborted, reset, interrupted) instead of dying mid-run. Fatal errors are logged before the acceptor stops, and a unit test pins the classification.

## [0.7.0] - 2026-10-08

### Added

- `ls` diagnoses a 400 from a repository-scoped search: the new `SearchRepoMissing` error carries the hint `the repository is missing on the server or is not a raw repository`, while every other endpoint keeps the generic status hint.
- The `search-400` scenario in `mock-nexus`: the search API answers 400 when the `repository` parameter names an unknown repository, like a real Nexus refusing a repository-scoped search, pinned by core, CLI and mock tests.
- `diff(local_dir, remote_url, opts)` in the npm bindings: the four-way delta report (`same` / `missing-local` / `missing-remote` / `diverged`) with per-side digest and size facts, shaped like the CLI's `diff --json`.
- The wasm sandbox: the fake Nexus moved into a ServiceWorker, so the page runs on static hosting with no backend at an origin root over https or localhost. The upstream proxy gains an upstream allowlist, rate limiting and response size caps. CI gates the wasm read surface.

## [0.6.0] - 2026-10-07

### Added

- `nxr complete <shell>`: prints a shell completion script (bash, zsh, fish, powershell) to stdout.
The script is generated from the same clap tree the binary runs on and is pinned by golden tests.

- The web panel example (`examples/panel`) grows into a full demo stand: progress bars over SSE, toasts and inline errors, keyboard navigation, direct file links, single-file uploads (`POST /api/put`), two-step deletes (`POST /api/rm` with a dry-run plan), and the `just panel` recipe that serves the mock, seeds a tree and starts the panel.
The page caps its own job cards and log lines, and refuses server URLs with embedded credentials.
The server refuses cross-origin POSTs and upload targets outside `/repository/<name>/`.

- `nxr diff <local-dir> <url>`: the delta of a local directory against a storage enumeration (`manifest.json` by convention, `--manifest`, `--name`, `--ls`), printed as `same` / `missing-local` / `missing-remote` / `diverged` (size and/or sha) without writing anything.
Exit 0 when equal, 1 when different, 2 and 3 for misuse and transport.
`--json` prints one object per entry and is pinned by a golden test.

- The `nxr-tui` browser uploads: `p` puts a local directory into the current tree position with marker-complete `up` semantics, one transfer panel shows the plan, the byte progress and the outcome, `x` cancels, and dialogs survive background failures.
- Dual-pane: `v` opens the local folder beside the remote tree, `h` and `l` switch the focus, `d` anchors at the local pane, `p` pre-fills the source.
- `space` marks rows and `d` transfers the marks as a queue.
- The pickers remember the confirmed values, `up` and `down` page through them.
### Changed

- `up --plan` and `down --plan` print the planned actions without transferring anything.
The old `--dry-run` spelling keeps working as a flag alias.

## [0.5.0] - 2026-10-06

### Added

- The `nxr-tui` terminal browser joins as the `nexus-raw-tui` crate.
One tab per server, the raw tree at any depth, subtree downloads, a live filter and server presets.
`enter` on a file opens a card with the size, the digest and the URL, and downloads start from the card or from `d`.
Downloads land in an explicitly picked destination folder (`o`), shown in the status bar, with a dialog form (`D`) that previews the target path.
Failures open an error modal with a plain-language cause, the core hint, and a clipboard copy of the full text.
- `nxr mv <src-url> <dst-url>`: renames a subtree as `mirror` plus `rm`, and nothing is deleted until the copy converges on the destination.
- `--prefix <path>` on `down`, `mirror`, `rm` and `mv`: scope the operation to one subtree of the repository.
- `nxr service repos`: lists the repositories of the Nexus instance with their format and writability.
The Node addon exposes the same call as `serviceRepos`.
- `ls_entries()` in the core and `lsEntries` in the Node addon: the direct children of any directory URL at any depth.
Directories come first, and `.sha256` marker siblings are hidden as derived data.

### Changed

- The TUI crate ships as `nexus-raw-tui`.
The binary inside stays `nxr-tui`, and so do its config paths and smoke directory names.
- `nxr ls <url>` prints the raw tree: directories with a trailing slash, files bare, at any depth.
- The version view left the CLI and remains in the library API as `ls_versions`.
`--assets` still lists the artifacts of a version.
- The human and NDJSON render contract is pinned by golden tests and documented in the CLI reference.
- The TUI and panel examples navigate the same raw tree: fail-fast startup, raw-only entry by default and browsing at any depth.

### Fixed

- The real-Nexus battery checks the entry listing (a version folder) instead of the old version-only listing.
- Uppercase letters type into every TUI input field: a real terminal delivers capitals with the Shift modifier, and the fields ignored them.
- The bundled Inter and Space Grotesk webfonts ship their SIL OFL 1.1 license texts.

## [0.4.0] - 2026-10-04

### Added

- The transport honors `429 Too Many Requests` with `Retry-After`: rate-limited requests replay on the server's schedule, pinned by a mock scenario and a golden `retrying` event.
- `nxr down --dry-run` and `nxr mirror --dry-run`: probe both sides and print the plan (`download`/`copy`/`skip`) without moving a byte.
- `nxr mirror --src-user USER:PASS` and `--dst-user USER:PASS`: credentials for one side only, overriding the shared `-u`.
- The Node addon reaches CLI parity: `rm` with dry-run planning, `pointClear`, per-side mirror credentials, named ESM imports.
A thrown event-callback error rejects the run's promise.
- Four mock scenarios (`rate-limit`, `auth-403`, `redirect`, `cut-body`) bring the conformance table to sixteen rows.
HEAD answers follow RFC 9110, and an oversized head is a recognizable `400`.

### Changed

- The human byte progress draws one in-place line and erases it cleanly.
Pipes stay byte-clean.
- The doctor names proxies and flags plaintext credentials in the environment.
- `verify` prints its verdict after the summary.
`head` escapes server-supplied strings.
`get` refuses `--json` while the body goes to stdout.
- `down` refuses symlinked intermediate directories, skips an unfinished download beside its target and surfaces scan entry errors.
Repeated explicit names collapse to one transfer.
- `put` hashes on the wire instead of re-reading the file.
The mirror writes through the destination client with a private staging directory.
- Every request answers within the stall budget, and the error surface is slimmer with exit codes unchanged.
- The declared MSRV follows the locked tree to 1.88.

## [0.3.0] - 2026-10-03

### Added

- `nxr mirror --src --dst`: pours the enumerated names from one raw repository into another.
The version document transfers first and alone.
- `nxr rm`: deletes the marker first, then the object.
- `nxr point --clear`: deletes the pointer file.
- Downloads decode `Content-Encoding: zstd`.
Resumed downloads pin identity coding so digest math stays exact across Range parts.
- The mock server models group repositories: a read-only aggregation that forwards to its members and refuses writes with `405`, pinned by both conformance suites.

### Changed

- An idiomatization pass across core, CLI and mock: the facade slimmed, the mock split into facade, store and tests, `# Errors` sections on every fallible public API.
- The npm publish job stages the generated loader, and a dry run rehearses the main package staging.
The merge gate runs the full `just check`.

## [0.2.0] - 2026-10-03

### Added

- The npm channel: the main `nexus-raw` package plus three prebuilt platform packages (`linux-x64-gnu`, `darwin-x64`, `darwin-arm64`), published through npm trusted publishing with no static token.

### Changed

- The transport moves to reqwest 0.13, and toml, base64 and sha2 follow their current majors.
- The release workflow publishes the node packages, dry runs rehearse the staging, and a publish loop guards against a partial publish.
- The node tracks run on pnpm.
The attach job names its repository without a checkout.
- The real-Nexus stand runs nightly against a live Nexus 3.

## [0.1.1] - 2026-10-02

### Added

- Platform binaries for four targets plus `SHA256SUMS` attached to the GitHub release.
- A `just version` recipe that asserts every copy of the release version, checked again on the tag.

### Fixed

- `nxr ls --assets` skips the reserved `.sha256` sibling suffix.
- `just nxr` survives a leading double dash.

## [0.1.0] - 2026-10-01

### Added

- The protocol core: get/put/head primitives, symmetric diff, two-phase `up`/`down` with markers and digest verification, resume by default with digest-guarded part restarts, Range 206/416, 429 backoff and stall detection.
- Channels and pointer tokens: dotted-numeric `--if-forward` ordering for `latest`-style names.
- The `nxr` CLI: six commands over the core, NDJSON output, exit codes 0 ok / 1 data / 2 misuse / 3 transport, credentials never printed.
- The `mock-nexus` server with nine failure scenarios as the conformance contract.
- The documentation site with README in English and Russian.

[Unreleased]: https://github.com/Lipen/nexus-raw/compare/v0.24.1...HEAD
[0.24.1]: https://github.com/Lipen/nexus-raw/compare/v0.24.0...v0.24.1
[0.24.0]: https://github.com/Lipen/nexus-raw/compare/v0.23.0...v0.24.0
[0.23.0]: https://github.com/Lipen/nexus-raw/compare/v0.22.1...v0.23.0
[0.22.1]: https://github.com/Lipen/nexus-raw/compare/v0.22.0...v0.22.1
[0.22.0]: https://github.com/Lipen/nexus-raw/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/Lipen/nexus-raw/compare/v0.20.0...v0.21.0
[0.20.0]: https://github.com/Lipen/nexus-raw/compare/v0.19.1...v0.20.0
[0.19.1]: https://github.com/Lipen/nexus-raw/compare/v0.19.0...v0.19.1
[0.19.0]: https://github.com/Lipen/nexus-raw/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/Lipen/nexus-raw/compare/v0.17.0...v0.18.0
[0.17.0]: https://github.com/Lipen/nexus-raw/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/Lipen/nexus-raw/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/Lipen/nexus-raw/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/Lipen/nexus-raw/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/Lipen/nexus-raw/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/Lipen/nexus-raw/compare/v0.11.1...v0.12.0
[0.11.1]: https://github.com/Lipen/nexus-raw/compare/v0.11.0...v0.11.1
[0.11.0]: https://github.com/Lipen/nexus-raw/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/Lipen/nexus-raw/compare/v0.9.0...v0.10.0
[0.9.0]: https://github.com/Lipen/nexus-raw/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/Lipen/nexus-raw/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/Lipen/nexus-raw/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/Lipen/nexus-raw/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Lipen/nexus-raw/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Lipen/nexus-raw/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Lipen/nexus-raw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Lipen/nexus-raw/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Lipen/nexus-raw/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Lipen/nexus-raw/commits/v0.1.0
