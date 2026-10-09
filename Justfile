# nexus-raw developer entry: `just <recipe>`, `cargo run -p mock-nexus -- atomic`, or `nxr <cmd>`.

[private]
default:
    @just --list

# Tool switches, one place to change them.
cargo := "cargo"
prek := "uvx prek"

# Usage example: `just cargo test -p nexus-raw-core`.
[doc('Run any cargo subcommand with flags.')]
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
# Usage: `just mock atomic --port 8080`.
[doc('Serve the mock Nexus.')]
[group('mock')]
mock scenario='atomic' *args:
    {{cargo}} run -p mock-nexus --quiet -- {{scenario}} {{args}}

# The real-Nexus stand: a dockerized Nexus3, bootstrapped, driven by the same
# battery the nightly job runs. Needs docker; the recipe dumps the server log
# on failure and leaves no state behind either way.
# Boot, bootstrap (waits, accepts the EULA, creates the repo), the 24-check battery, cleanup on any exit.
[doc('Run the real-Nexus stand against a local docker Nexus.')]
[group('stand')]
stand *args:
    stand/real-nexus/run.sh {{args}}

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

# ---- bench -----------------------------------------------------------------

# Opt-in and manual: bench/ is a standalone workspace the gate never compiles.
# A bare name selects one scenario: `just bench scan`. Everything else passes
# through to cargo, so `just bench scan -- --save-baseline before` reaches
# criterion. A typo must fail loudly, never run nothing: cargo treats an
# unknown bare word as a TESTNAME filter and exits 0 having run nothing, so
# only the four known names select a target and any other bare word is
# refused. --locked keeps a comparison free of a silent re-lock between its
# before and after sides.
# `just bench` is all, `just bench scan` is one.
[doc('Run the criterion benches.')]
[group('bench')]
bench *args:
    #!/bin/sh
    set -eu
    set -- {{ args }}
    case "${1:-}" in
        scan|diff|up|down) set -- --bench "$@" ;;
        -*) ;;
        *) echo "bench: unknown scenario '${1}' (want scan, diff, up or down)" >&2; exit 2 ;;
    esac
    exec cargo bench --locked --manifest-path bench/Cargo.toml --quiet "$@"

# ---- release ---------------------------------------------------------------

# The workspace manifest is the source of truth.
# The npm package, the internal dependency pins and the two standalone example locks follow it, their resolutions included.
# examples/node cannot follow before the registry has the release, so the recipe ends with the `just lock` reminder.
# `just check` runs version-check.
# Usage: `just version 0.2.0`.
[doc('Set the release version everywhere it is asserted.')]
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
    perl -pi -e "s/(nexus-raw-core = \{ path = \"[^\"]*\", version = )\"[^\"]*\"/\${1}\"{{v}}\"/" crates/nexus-raw/Cargo.toml
    perl -pi -e "s/(nexus-raw-core = \{ path = \"[^\"]*\", version = )\"[^\"]*\"/\${1}\"{{v}}\"/" crates/nexus-raw-tui/Cargo.toml
    # The node example's manifest pin is a string, safe to move before the
    # release; its lockfile resolves only after the registry has the version
    # and stays on the post-release `just lock` path.
    perl -pi -e "s/(\"nexus-raw\": \")\\^[^\"]*(\")/\${1}\\^{{v}}\${2}/" examples/node/package.json
    # The node example's lockfile specifier moves with the manifest, so the
    # CI `pnpm ci` (frozen) stays green between the bump and the post-release
    # `just lock` that re-resolves the lock against the published registry.
    perl -pi -e "s/^(        specifier: )\\^[^\\n]*/\${1}\\^{{v}}/" examples/node/pnpm-lock.yaml
    # A silent perl miss is the 0.6.0 release killer: assert every pin.
    grep -q "version = \"{{v}}\"" crates/nexus-raw/Cargo.toml || { echo "version: the nexus-raw dep pin did not move" >&2; exit 1; }
    grep -q "version = \"{{v}}\"" crates/nexus-raw-tui/Cargo.toml || { echo "version: the tui dep pin did not move" >&2; exit 1; }
    grep -q "\"version\": \"{{v}}\"" crates/nexus-raw-napi/package.json || { echo "version: the napi package version did not move" >&2; exit 1; }
    grep -q "specifier: ^{{v}}" examples/node/pnpm-lock.yaml || { echo "version: the node lock specifier did not move" >&2; exit 1; }
    grep -q "\"nexus-raw\": \"^{{v}}\"" examples/node/package.json || { echo "version: the node example pin did not move" >&2; exit 1; }
    # Resolve so every lock follows the new workspace version: the root lock first, then the two standalone example locks.
    # examples/node waits for the registry: `just lock` refreshes it after the release.
    cargo metadata --format-version 1 >/dev/null
    cargo metadata --manifest-path examples/rust/Cargo.toml --format-version 1 >/dev/null
    cargo metadata --manifest-path examples/wasm-sandbox/Cargo.toml --format-version 1 >/dev/null
    # Assert after resolving: the example locks agree only once their own resolution has run.
    # The workspace pins are already asserted above, so a drifted pin still fails before any resolver does.
    scripts/version-check.sh
    echo "version {{v}} set (was $old)"
    echo "remind: run just lock after the release: it refreshes every lockfile, examples/node against the registry"

# The manifest is the source of truth for the current number; this only does
# the arithmetic and hands the result to `just version`.
# `patch`, `minor` or `major`: `just bump minor`.
[doc('Compute the next version and set it.')]
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

# The whole release act: bump, changelog, push, dispatch, watch. A repeat
# call for the same number is the recovery path: the version step is
# idempotent, the changelog date refreshes to today, the guards in the
# workflow skip whatever is already published.
# The real run bumps, pushes, publishes and tags: `just release 0.6.0 --yes`.
[doc('Release X.Y.Z: a preview by default, --yes for the real run.')]
[group('release')]
release v *mode:
    #!/bin/sh
    set -eu
    case "{{v}}" in
      [0-9]*.[0-9]*.[0-9]*) ;;
      *) echo "release: {{v}} is not X.Y.Z" >&2; exit 2 ;;
    esac
    yes=""
    for m in {{mode}}; do
      case "$m" in
        --yes) yes=1 ;;
        *) echo "release: unknown mode '$m' (want --yes)" >&2; exit 2 ;;
      esac
    done
    if [ -z "$yes" ]; then
      old="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
      echo "release {{v}}: dry run, nothing is pushed or published"
      if [ "$old" = "{{v}}" ]; then
        echo "  version: already {{v}}, the bump would be a no-op"
      else
        echo "  version: $old -> {{v}} in Cargo.toml, the napi package.json, two dependency pins, Cargo.lock"
      fi
      python3 scripts/changelog-finalize.py "{{v}}" --dry | sed 's/^/  /'
      if [ -n "$(git status --porcelain)" ]; then
        echo "  warning: the tree is dirty, --yes would commit the tracked changes"
      fi
      if git ls-remote --exit-code origin "refs/tags/v{{v}}" > /dev/null 2>&1; then
        echo "  warning: tag v{{v}} already exists, --yes would fail at the gate"
      fi
      if curl -sf "https://index.crates.io/ne/xu/nexus-raw-core" | grep -q "\"vers\":\"{{v}}\""; then
        echo "  note: {{v}} is already on crates.io, the crates job would skip"
      fi
      echo "  pipeline: gate -> builds (linux-x64, linux-aarch64, darwin-x64, darwin-arm64, windows-x64) -> crates -> node addons -> npm -> tag v{{v}} + release"
      echo "nothing was pushed or published: call 'just release {{v}} --yes' for the real act"
      exit 0
    fi
    just version "{{v}}"
    python3 scripts/changelog-finalize.py "{{v}}"
    git add -u
    if ! git diff --cached --quiet; then
      git commit -m "chore: bump the workspace to {{v}}"
    fi
    git push origin master
    sha="$(git rev-parse origin/master)"
    # The dispatch API takes a branch, not a raw SHA: master is the ref, and
    # the gate validates whatever HEAD it gets.
    echo "dispatching the release on $sha"
    gh workflow run release.yml --ref master
    run=""
    for i in 1 2 3 4 5 6; do
      sleep 5
      run="$(gh run list --workflow=Release --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
      [ -n "$run" ] && break
    done
    [ -n "$run" ] || { echo "release: the dispatched run did not appear" >&2; exit 1; }
    gh run watch "$run" --interval 30 --exit-status
    echo "released: https://github.com/Lipen/nexus-raw/releases/tag/v{{v}}"

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
build-release:
    {{cargo}} build --release -p nexus-raw

# Links the npm package, then claim-first up, channel, down and verify.
[doc('Run the Node example against the mock.')]
[group('examples')]
example-node:
    cargo build -q -p mock-nexus
    cargo build -q -p nexus-raw-napi
    cd examples/node && pnpm install && node publish-and-consume.mjs

# The panel stand, exactly the README's run: build the addon and the mock,
# serve the mock on :8099, seed a three-level tree, start the panel on :8123.
# The mock dies with the recipe: Ctrl-C the panel and both are gone.
# Builds the panel, seeds the mock, serves it.
[doc('Run the web panel against the mock on :8123.')]
[group('examples')]
panel *args:
    #!/bin/sh
    set -e
    cargo build -q -p mock-nexus -p nexus-raw -p nexus-raw-napi
    target/debug/mock-nexus atomic --port 8099 &
    mock=$!
    trap 'kill "$mock" 2>/dev/null || :' EXIT INT TERM
    i=0
    while ! curl -sf http://127.0.0.1:8099/service/rest/v1/repositories >/dev/null 2>&1; do
        i=$((i + 1))
        if [ "$i" -gt 100 ]; then
            echo 'panel: the mock did not answer on :8099 (a busy port?)' >&2
            exit 1
        fi
        sleep 0.1
    done
    rm -rf /tmp/panel-seed
    mkdir -p /tmp/panel-seed/app/core/util /tmp/panel-seed/app/bin /tmp/panel-seed/dist
    printf 'readme\n' > /tmp/panel-seed/README.txt
    printf 'lib\n' > /tmp/panel-seed/app/core/lib.rs
    printf 'math\n' > /tmp/panel-seed/app/core/util/math.rs
    printf 'shebang\n' > /tmp/panel-seed/app/bin/nxr.sh
    printf 'svg\n' > /tmp/panel-seed/dist/logo.svg
    target/debug/nxr up /tmp/panel-seed http://127.0.0.1:8099/repository/raw-main/
    printf '{"continuationToken":null,"items":[{"path":"README.txt"},{"path":"app/core/lib.rs"},{"path":"app/core/util/math.rs"},{"path":"app/bin/nxr.sh"},{"path":"dist/logo.svg"}]}' > /tmp/search-page.json
    target/debug/nxr put http://127.0.0.1:8099/service/rest/v1/search/assets -f /tmp/search-page.json
    cd examples/panel
    pnpm install
    node server.mjs --url http://127.0.0.1:8099/ {{args}}

# The wasm sandbox: the read surface of the core compiled for the browser.
# The first build needs the wasm32 target and wasm-bindgen-cli.
# `just wasm` serves the fake; `just wasm --upstream <url>` serves a real Nexus.
[doc('Build and serve the wasm sandbox.')]
[group('examples')]
wasm *args:
    cd examples/wasm-sandbox && cargo build --target wasm32-unknown-unknown --release --locked
    cd examples/wasm-sandbox && wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/nexus_raw_example_wasm_sandbox.wasm
    cd examples/wasm-sandbox && node serve.mjs {{args}}

# Build the sandbox and run its smoke: the fake module, the wasm listing under
# node and the ServiceWorker page in a headless browser when one is around.
# This is what CI's wasm job runs; the browser layer is skipped where no
# chromium binary lives (set NXR_WASM_DRIVER or NXR_WASM_BROWSER to steer it).
# The fake module, the wasm listing and the page in a headless browser when one is available.
[doc('Build and smoke the wasm sandbox.')]
[group('examples')]
wasm-smoke:
    cd examples/wasm-sandbox && cargo build --target wasm32-unknown-unknown --release --locked
    cd examples/wasm-sandbox && wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/nexus_raw_example_wasm_sandbox.wasm
    cd examples/wasm-sandbox && node smoke.mjs

# Every lockfile in the repo, one deliberate command: the workspace, the napi
# tooling, the panel, the two standalone example crates and the node example.
# The node example resolves against the registry, so it only refreshes once
# the released version is actually published; before a release that lock is
# expected to stay stale. Run after every release, commit the result.
# Workspace, napi, panel, node (post-release), rust and sandbox examples.
[doc('Refresh every lockfile in the repo.')]
[group('examples')]
lock:
    #!/bin/sh
    set -eu
    cargo metadata --format-version 1 > /dev/null
    echo "workspace Cargo.lock: refreshed"
    (cd crates/nexus-raw-napi && pnpm install --no-frozen-lockfile --reporter=silent)
    echo "napi pnpm-lock: refreshed"
    (cd examples/panel && pnpm install --no-frozen-lockfile --reporter=silent)
    echo "panel pnpm-lock: refreshed"
    cargo metadata --manifest-path examples/rust/Cargo.toml --format-version 1 > /dev/null
    echo "examples/rust Cargo.lock: refreshed"
    cargo metadata --manifest-path examples/wasm-sandbox/Cargo.toml --format-version 1 > /dev/null
    echo "examples/wasm-sandbox Cargo.lock: refreshed"
    v="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
    if curl -sf https://registry.npmjs.org/nexus-raw | grep -q "\"latest\":\"$v\""; then
      (cd examples/node && pnpm up "nexus-raw@^$v" --reporter=silent)
      echo "examples/node pnpm-lock: refreshed against $v"
    else
      echo "examples/node pnpm-lock: skipped, the registry does not have $v yet (normal before a release)"
    fi
    echo "---"
    git status --short | grep -E "Cargo.lock|pnpm-lock.yaml" || echo "everything was already fresh"

[doc('Run the Rust example: standalone crate on a path dependency.')]
[group('examples')]
example-rust:
    cargo build -q -p mock-nexus
    cargo run --manifest-path examples/rust/Cargo.toml

# The local stand: mock server, stub payloads, the real binary. Every command
# is printed as it runs; scenarios are the mock's (`just mock --print-scenarios`).
# Publish, name, consume, verify, refuse and repair, in order.
[doc('Run the demo stand scenario.')]
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
# The cast file, its pace and the landing SVG.
[doc('Record the demo session.')]
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
[doc('Build the docs, check llms.txt and fail on issues.')]
[group('docs')]
check-docs *args:
    scripts/llms-check.sh
    uvx zensical build --strict {{args}}
