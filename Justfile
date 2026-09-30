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
[doc('Run the nxr CLI against your base URL.')]
nxr *args:
    {{cargo}} run -p nexus-raw --quiet -- {{args}}

# `--auth user:pass` and scenario flags come after the scenario name.
[doc('Serve the mock Nexus: `just mock atomic --port 8080`.')]
[group('mock')]
mock scenario='atomic' *args:
    {{cargo}} run -p mock-nexus --quiet -- {{scenario}} {{args}}

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
[doc('The whole gate: fmt + clippy + prek + tests.')]
[group('check')]
check:
    just fmt -- --check
    just clippy
    just lint
    just test

[doc('Debug build of the workspace.')]
[group('build')]
build *args:
    {{cargo}} build --workspace {{args}}

[doc('Release build of the nxr binary (static-friendly, stripped).')]
[group('build')]
release:
    {{cargo}} build --release -p nexus-raw

[doc('Run the Node example: file: dependency, claim-first up, channel, down, verify.')]
[group('examples')]
example-node:
    cargo build -q -p mock-nexus
    cargo build -q -p nexus-raw-napi
    cd examples/node && npm install --silent && node publish-and-consume.mjs

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
[doc('Record the demo session into the docs landing animation.')]
[group('examples')]
demo-cast *args:
    cargo build -q -p mock-nexus -p nexus-raw
    NXR_DEMO_EXTRA=0 NXR_DEMO_CAST=docs/assets/cast/session.json \
        examples/demo/demo.sh slow --port 8734 --chunk-delay-ms 40 {{args}}
    test -s docs/assets/cast/session.json
    @echo "cast: docs/assets/cast/session.json"

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
