# Contributing

The repository is a Rust workspace (`crates/`), a docs site (`docs/`) and three examples (`examples/`).
`AGENTS.md` holds the rules that matter when you change behaviour. This file holds how to build, test and release.

## Build and test

```bash
just check        # fmt + clippy + prek + version-check + tests: the gate
just test         # unit and conformance suites alone
just check-docs   # the docs site builds and every link resolves
just demo         # a real session against the mock server and stub payloads
```

`just` is optional: every recipe is a thin wrapper over `cargo` or `uvx zensical`.

The conformance suites drive both `nexus-raw-core` and the `nxr` binary against `mock-nexus`, the failure-injection server in `crates/mock-nexus`.
Its scenario list is the contract: a scenario added for one client lands with the test that exercises it.

## Commits

Conventional, subject-only, no body, one change per commit: `scope: short imperative summary` with scopes `core`, `cli`, `mock`, `docs`, `chore`.
Commits go straight to master.
Commits must be signed: the remote rejects unsigned pushes.

## Changing the protocol

The protocol text is the contract, and a change to it is one change, not a series:

- update `docs/reference/protocol.md`;
- update every implementation in the same commit range: `nexus-raw-core`, the `nxr` surface, the Node bindings;
- add the mock scenario that exercises the new behaviour, in `crates/mock-nexus/src/lib.rs`, with its test;
- regenerate the recorded session and its animation (`just demo-cast`) when the demo's commands or the output format changed.

Docs are corrected in the same commit as the behaviour they describe.
Transcripts in the docs come from a real run: paste what the binary printed, never what it should have printed.

## Versioning

One version for the whole workspace, `[workspace.package] version` in `Cargo.toml`.
`nexus-raw-core`, `nexus-raw` and `mock-nexus` inherit it; `nexus-raw-napi` inherits it too, because the npm package carries the same number.

Before 1.0 a minor release may break anything. A patch release may not.
The public surface is larger than the Rust API: CLI flags and their defaults, exit codes 0/1/2/3, the `--json` shapes, the `hint:` text the docs quote, and the wire invariants (the `<name>.sha256` marker, the enumeration document, claim-first, the refusal to overwrite a diverged object).
Raising the MSRV is a minor change and belongs in the release notes.

```bash
just version 0.2.0      # sets it everywhere it is asserted
just version-check      # asserts they agree, and that no page pins a number
```

Pages must never state the current version: a pinned number is a lie on the next release.
The recorded session in `docs/assets/cast/` is the exception — it names the binary that made it, and `just demo-cast` refreshes it.

## Release

```bash
just check                        # green on master
just version X.Y.Z
git commit -am "chore: release X.Y.Z"
git tag -s vX.Y.Z -m "vX.Y.Z"
git push origin master --follow-tags
```

The tag starts `.github/workflows/release.yml`: it refuses a tag that disagrees with the workspace version, publishes `nexus-raw-core`, then `mock-nexus`, then `nexus-raw` with the `CARGO_REGISTRY_TOKEN` repository secret, and opens the GitHub release.
The generated release notes are the changelog. There is no `CHANGELOG.md` to rot.

`nexus-raw-napi` is `publish = false`.
Publishing the npm package is a separate track: it needs the per-platform prebuilt packages and the `optionalDependencies` wiring before a linux-only package can be avoided.

## License

MIT, see [LICENSE](LICENSE).
