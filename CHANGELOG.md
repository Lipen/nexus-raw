# Changelog

All notable changes to the `nxr` CLI, the `nexus-raw-core` library and the `mock-nexus` server are recorded here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `nxr mv <src-url> <dst-url>`: renames a subtree as `mirror` plus `rm`, and nothing is deleted until the copy converges on the destination.
- `--prefix <path>` on `down`, `mirror`, `rm` and `mv`: scope the operation to one subtree of the repository.
- `nxr service repos`: lists the repositories of the Nexus instance with their format and writability; the Node addon exposes the same call as `serviceRepos`.
- `ls_entries()` in the core and `lsEntries` in the Node addon: the direct children of any directory URL at any depth, directories first, with `.sha256` marker siblings hidden as derived data.

### Changed

- `nxr ls <url>` prints the raw tree: directories with a trailing slash, files bare, at any depth.
- The version view left the CLI and remains in the library API as `ls_versions`; `--assets` still lists the artifacts of a version.
- The human and NDJSON render contract is pinned by golden tests and documented in the CLI reference.
- The TUI and panel examples navigate the same raw tree: fail-fast startup, raw-only entry by default and browsing at any depth.

### Fixed

- The real-Nexus battery checks the entry listing (a version folder) instead of the old version-only listing.
- The bundled Inter and Space Grotesk webfonts ship their SIL OFL 1.1 license texts.

## [0.4.0] - 2026-10-04

### Added

- The transport honors `429 Too Many Requests` with `Retry-After`: rate-limited requests replay on the server's schedule, pinned by a mock scenario and a golden `retrying` event.
- `nxr down --dry-run` and `nxr mirror --dry-run`: probe both sides and print the plan (`download`/`copy`/`skip`) without moving a byte.
- `nxr mirror --src-user USER:PASS` and `--dst-user USER:PASS`: credentials for one side only, overriding the shared `-u`.
- The Node addon reaches CLI parity: `rm` with dry-run planning, `pointClear`, per-side mirror credentials, named ESM imports; a thrown event-callback error rejects the run's promise.
- Four mock scenarios (`rate-limit`, `auth-403`, `redirect`, `cut-body`) bring the conformance table to sixteen rows; HEAD answers follow RFC 9110, an oversized head is a recognizable `400`.

### Changed

- The human byte progress draws one in-place line and erases it cleanly; pipes stay byte-clean.
- The doctor names proxies and flags plaintext credentials in the environment.
- `verify` prints its verdict after the summary; `head` escapes server-supplied strings; `get` refuses `--json` while the body goes to stdout.
- `down` refuses symlinked intermediate directories, skips an unfinished download beside its target and surfaces scan entry errors; repeated explicit names collapse to one transfer.
- `put` hashes on the wire instead of re-reading the file; the mirror writes through the destination client with a private staging directory.
- Every request answers within the stall budget, and the error surface is slimmer with exit codes unchanged.
- The declared MSRV follows the locked tree to 1.88.

## [0.3.0] - 2026-10-03

### Added

- `nxr mirror --src --dst`: pours the enumerated names from one raw repository into another; the version document transfers first and alone.
- `nxr rm`: deletes the marker first, then the object.
- `nxr point --clear`: deletes the pointer file.
- Downloads decode `Content-Encoding: zstd`; resumed downloads pin identity coding so digest math stays exact across Range parts.
- The mock server models group repositories: a read-only aggregation that forwards to its members and refuses writes with `405`, pinned by both conformance suites.

### Changed

- An idiomatization pass across core, CLI and mock: the facade slimmed, the mock split into facade, store and tests, `# Errors` sections on every fallible public API.
- The npm publish job stages the generated loader, and a dry run rehearses the main package staging; the merge gate runs the full `just check`.

## [0.2.0] - 2026-10-03

### Added

- The npm channel: the main `nexus-raw` package plus three prebuilt platform packages (`linux-x64-gnu`, `darwin-x64`, `darwin-arm64`), published through npm trusted publishing with no static token.

### Changed

- The transport moves to reqwest 0.13, and toml, base64 and sha2 follow their current majors.
- The release workflow publishes the node packages, dry runs rehearse the staging, and a publish loop guards against a partial publish.
- The node tracks run on pnpm; the attach job names its repository without a checkout.
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

[Unreleased]: https://github.com/Lipen/nexus-raw/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/Lipen/nexus-raw/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Lipen/nexus-raw/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Lipen/nexus-raw/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/Lipen/nexus-raw/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/Lipen/nexus-raw/commits/v0.1.0
