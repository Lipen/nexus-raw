# Changelog

All notable changes to the `nxr` CLI, the `nexus-raw-core` library, the `nexus-raw-tui` browser and the `mock-nexus` server are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The npm package is renamed: `nexus-raw` becomes `@nexus-raw/nxr`, and the platform addons become `@nexus-raw/<platform>` (linux-x64-gnu, linux-arm64-gnu, darwin-x64, darwin-arm64).
  The publish rides GitHub OIDC trusted publishing, no tokens.
  The old unscoped name will be deprecated after this release settles; the import and the install line change with the name.

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

- The mock server rides out transient `accept()` errors (connection aborted, reset, interrupted) instead of dying mid-run; fatal errors are logged before the acceptor stops, and a unit test pins the classification.

## [0.7.0] - 2026-10-08

### Added

- `ls` diagnoses a 400 from a repository-scoped search: the new `SearchRepoMissing` error carries the hint `the repository is missing on the server or is not a raw repository`, while every other endpoint keeps the generic status hint.
- The `search-400` scenario in `mock-nexus`: the search API answers 400 when the `repository` parameter names an unknown repository, like a real Nexus refusing a repository-scoped search, pinned by core, CLI and mock tests.
- `diff(local_dir, remote_url, opts)` in the npm bindings: the four-way delta report (`same` / `missing-local` / `missing-remote` / `diverged`) with per-side digest and size facts, shaped like the CLI's `diff --json`.
- The wasm sandbox: the fake Nexus moved into a ServiceWorker, so the page runs on static hosting with no backend at an origin root over https or localhost; the upstream proxy gains an upstream allowlist, rate limiting and response size caps; CI gates the wasm read surface.

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

[Unreleased]: https://github.com/Lipen/nexus-raw/compare/v0.10.0...HEAD
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
