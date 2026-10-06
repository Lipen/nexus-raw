# nexus-raw developer entry: `just <recipe>`, `cargo run -p mock-nexus -- atomic`, or `nxr <cmd>`.

[private]
default:
    @just --list

# Tool switches, one place to change them.
cargo := "cargo"
prek := "uvx prek"

[doc('Run any cargo subcommand with flags: `just cargo test -p nexus-raw-core`.')]
cargo *args:
    {{cargo}} {{args}}

# Runs the binary: `just nxr -- up --base http://127.0.0.1:8080/ --dir dist/`.
# A leading `--` is the habit cargo and curl teach; one of those is swallowed
# so the CLI never sees it, while the recipe's own `--` keeps cargo's hands
# off the flags: without it `just nxr --version` would print cargo's version.
# Args pass through word-splitting (a just interpolation limit), so paths
# with spaces need `cargo run -p nexus-raw --` directly.
[doc('Run the nxr CLI against your base URL.')]
nxr *args:
    @set -- {{args}} && { [ "${1:-}" = -- ] && shift || :; } && \
        exec {{cargo}} run -p nexus-raw --quiet -- "$@"

# The same passthrough for the terminal browser: `just tui http://127.0.0.1:8081/`,
# `just tui -- --smoke http://127.0.0.1:8081/`, `just tui -- --init-config`.
[doc('Run the nxr-tui browser against your base URL.')]
tui *args:
    @set -- {{args}} && { [ "${1:-}" = -- ] && shift || :; } && \
        exec {{cargo}} run -p nexus-raw-tui --quiet -- "$@"

# `--auth user:pass` and scenario flags come after the scenario name.
[doc('Serve the mock Nexus: `just mock atomic --port 8080`.')]
[group('mock')]
mock scenario='atomic' *args:
    {{cargo}} run -p mock-nexus --quiet -- {{scenario}} {{args}}

# The real-Nexus stand: a dockerized Nexus3, bootstrapped, driven by the same
# battery the nightly job runs. Needs docker; the recipe dumps the server log
# on failure and leaves no state behind either way.
[doc('Run the real-Nexus stand: docker nexus3, bootstrap, the battery.')]
[group('stand')]
stand *args:
    cargo build -q --locked -p nexus-raw && \
        docker compose -f stand/real-nexus/compose.yaml up -d && \
        stand/real-nexus/bootstrap.sh && \
        stand/real-nexus/battery.sh {{args}}; status=$?; \
    if [ "$status" -ne 0 ]; then docker compose -f stand/real-nexus/compose.yaml logs --tail 200 >&2 || :; fi; \
    docker compose -f stand/real-nexus/compose.yaml down -v; exit $status

[doc('Format all crates.')]
[group('check')]
fmt *args:
    {{cargo}} fmt --all {{args}}

# The lint contract: warnings are errors, same as the prek hook.
[doc('Clippy over the workspace, warnings are errors.')]
[group('check')]
clippy *args:
    {{cargo}} clippy --workspace --all-targets -- -D warnings {{args}}

# Runs the same hooks the git hook runs (prek.toml).
[doc('Lint, format-check and hygiene hooks through prek.')]
[group('check')]
lint *args:
    {{prek}} run --all-files {{args}}

[doc('Unit and conformance tests (cargo test).')]
[group('check')]
test *args:
    {{cargo}} test --workspace {{args}}

# The full gate a change must pass before it lands: fmt, clippy, prek, tests.
[doc('The whole gate: fmt + clippy + prek + tests + version agreement.')]
[group('check')]
check:
    just fmt -- --check
    just clippy
    just lint
    just version-check
    just test

# ---- release ---------------------------------------------------------------

# The workspace manifest is the source of truth; the npm package, the internal
# dependency versions and the example pins follow it. The example locks cannot
# follow before the registry has the release, so the recipe ends with the
# post-publish reminder. `just check` runs version-check.
[doc('Set the release version everywhere it is asserted: `just version 0.2.0`.')]
[group('release')]
version v:
    #!/bin/sh
    set -eu
    case "{{v}}" in
      [0-9]*.[0-9]*.[0-9]*) ;;
      *) echo "version {{v}} is not X.Y.Z" >&2; exit 2 ;;
    esac
    old="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
    [ -n "$old" ] || { echo "no workspace version in Cargo.toml" >&2; exit 1; }
    perl -pi -e "s/^version = \"[^\"]*\"/version = \"{{v}}\"/" Cargo.toml
    perl -pi -e "s/\Q$old\E/{{v}}/g" crates/nexus-raw-napi/package.json
    perl -pi -e "s/version = \"\Q$old\E\"/version = \"{{v}}\"/" crates/nexus-raw/Cargo.toml
    perl -pi -e "s/(nexus-raw-core = \{ path = \".\/nexus-raw-core\", version = )\"[^\"]*\"/\${1}\"{{v}}\"/" crates/nexus-raw-tui/Cargo.toml
    # Resolve once so Cargo.lock carries the new workspace version.
    cargo metadata --format-version 1 >/dev/null
    # The recipe owns every asserted copy: a failed edit must fail the recipe,
    # not wait for `just check` to find a half-bumped tree.
    scripts/version-check.sh
    echo "version {{v}} set (was $old)"
    echo "remind: the example locks resolve only after the registry has {{v}}:"
    echo "  cd examples/node && pnpm up nexus-raw@^$(echo "{{v}}" | cut -d. -f1-2).0"

# The manifest is the source of truth for the current number; this only does
# the arithmetic and hands the result to `just version`.
[doc('Compute the next version and set it: `just bump patch` (also minor, major).')]
[group('release')]
bump part:
    #!/bin/sh
    set -eu
    cur="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
    [ -n "$cur" ] || { echo "bump: no workspace version in Cargo.toml" >&2; exit 1; }
    IFS=.
    set -- $cur
    [ "$#" -eq 3 ] || { echo "bump: '$cur' is not X.Y.Z" >&2; exit 2; }
    case "{{part}}" in
      patch) next="$1.$2.$(($3 + 1))" ;;
      minor) next="$1.$(($2 + 1)).0" ;;
      major) next="$(($1 + 1)).0.0" ;;
      *) echo "bump: '{{part}}' is not patch, minor or major" >&2; exit 2 ;;
    esac
    echo "bumping $cur -> $next"
    exec just version "$next"

[doc('Assert the version agrees everywhere and that no page pins it.')]
[group('release')]
[group('check')]
version-check:
    scripts/version-check.sh

[doc('Debug build of the workspace.')]
[group('build')]
build *args:
    {{cargo}} build --workspace {{args}}

[doc('Release build of the nxr binary (static-friendly, stripped).')]
[group('build')]
release:
    {{cargo}} build --release -p nexus-raw

[doc('Run the Node example: link: dependency, claim-first up, channel, down, verify.')]
[group('examples')]
example-node:
    cargo build -q -p mock-nexus
    cargo build -q -p nexus-raw-napi
    cd examples/node && pnpm install && node publish-and-consume.mjs

[doc('Run the Rust example: standalone crate on a path dependency.')]
[group('examples')]
example-rust:
    cargo build -q -p mock-nexus
    cargo run --manifest-path examples/rust/Cargo.toml

# The local stand: mock server, stub payloads, the real binary. Every command
# is printed as it runs; scenarios are the mock's (`just mock --print-scenarios`).
[doc('Run the demo stand: publish, name, consume, verify, refuse, repair.')]
[group('examples')]
demo scenario='slow' *args:
    cargo build -q -p mock-nexus -p nexus-raw
    examples/demo/demo.sh {{scenario}} {{args}}

# The landing animation is a recording of that same stand, not a mock-up.
# The port is pinned so the recorded URLs stay comparable between recordings;
# a busy port fails loudly instead of silently changing the file.
# Three steps, one source: the cast (a real run), the pace (a reader's timeline)
# and the SVG (the same timeline, drawn for pages without JavaScript).
# Needs python3 for the two tiny scripts next to the stand.
[doc('Record the demo session: the cast, its pace and the landing SVG.')]
[group('examples')]
demo-cast *args:
    cargo build -q -p mock-nexus -p nexus-raw
    NXR_DEMO_SCOPE=core NXR_DEMO_CAST=docs/assets/cast/session.json \
        examples/demo/demo.sh slow --port 8734 --chunk-delay-ms 5 {{args}}
    test -s docs/assets/cast/session.json
    python3 examples/demo/stage.py docs/assets/cast/session.json
    python3 examples/demo/svg.py docs/assets/cast/session.json docs/assets/img/session.svg
    @echo "cast: docs/assets/cast/session.json"
    @echo "svg:  docs/assets/img/session.svg"

# The whole site, written in docs/ (zensical, Material stack).
# Live reload included: edit a page, the browser refreshes itself.
[doc('Serve the docs site with live reload (http://localhost:8000).')]
[group('docs')]
docs *args:
    uvx zensical serve {{args}}

# A docs link or config typo fails the build instead of shipping broken pages.
[doc('Build the docs and fail on issues.')]
[group('docs')]
check-docs *args:
    uvx zensical build {{args}}
