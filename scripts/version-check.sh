#!/bin/sh
# Assert that every place carrying the release version agrees with the
# workspace manifest, and that no page pins a number.
#
# Called from `just check` and from CI: the version lives in one place, and a
# second copy that drifts is a broken release.
set -eu

ws="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$ws" ] || { echo "version-check: no workspace version in Cargo.toml" >&2; exit 1; }

pkg="$(sed -n 's/^  "version": "\(.*\)",/\1/p' crates/nexus-raw-napi/package.json)"
dep="$(sed -n 's/.*nexus-raw-core = { path = "\.\.\/nexus-raw-core", version = "\(.*\)" }.*/\1/p' crates/nexus-raw/Cargo.toml)"
tui="$(sed -n 's/.*nexus-raw-core = { path = "\.\.\/nexus-raw-core", version = "\(.*\)" }.*/\1/p' crates/nexus-raw-tui/Cargo.toml)"

failed=0
for pair in "crates/nexus-raw-napi/package.json:$pkg" "nexus-raw's dependency on nexus-raw-core:$dep" "nexus-raw-tui's dependency on nexus-raw-core:$tui"; do
  name="${pair%%:*}"; got="${pair#*:}"
  if [ "$got" != "$ws" ]; then
    echo "version-check: $name is '$got', the workspace is '$ws'" >&2
    failed=1
  fi
done

# The examples are standalone consumers pinned to the minor: a pin that
# lags the workspace teaches a version nobody released. The rust example
# carries the loose major.minor form, the node example the caret form.
# Between releases the rust example may ride the workspace as a path
# dependency instead (breaking core changes land on master first).
minor="${ws%.*}"
rust_pin="$(sed -n 's/^nexus-raw-core = "\(.*\)"/\1/p' examples/rust/Cargo.toml)"
rust_path="$(sed -n 's/^nexus-raw-core = { path = .*}/path/p' examples/rust/Cargo.toml)"
# The npm surface lives under @nexus-raw/nxr since the scoped rename.
# The example pins an exact placeholder until the scoped 1.0.0 ships, so the
# caret-shape check below is waived: the rename commit documents it.
node_pin="$(sed -n 's/.*"@nexus-raw\/nxr": "\(.*\)".*/\1/p' examples/node/package.json)"
if [ -n "$rust_path" ]; then
  : # the workspace build: nothing to agree with
elif [ -z "$rust_pin" ]; then
  echo "version-check: no nexus-raw-core pin in examples/rust/Cargo.toml" >&2
  failed=1
elif [ "$rust_pin" != "$minor" ]; then
  echo "version-check: examples/rust pins $rust_pin, the workspace is $minor" >&2
  failed=1
fi

# The standalone example locks must record nexus-raw-core at the workspace version: a lock left behind pins a number nobody released, and nothing rebuilds the lock until `just lock`.
# examples/node is exempt: its pnpm lock resolves only once the registry has the release.
for lock in examples/rust/Cargo.lock examples/wasm-sandbox/Cargo.lock; do
  lock_ver="$(sed -n '/^name = "nexus-raw-core"$/ { n; s/^version = "\(.*\)"$/\1/p }' "$lock")"
  if [ -z "$lock_ver" ]; then
    echo "version-check: no nexus-raw-core entry in $lock" >&2
    failed=1
  elif [ "$lock_ver" != "$ws" ]; then
    echo "version-check: $lock pins nexus-raw-core $lock_ver, the workspace is $ws" >&2
    failed=1
  fi
done

# Pages may name the command, never the current number: a pinned version is a
# lie on the next release. The recorded session under docs/assets is exempt:
# it names the binary that made the recording, and `just demo-cast` refreshes it.
# CHANGELOG.md is exempt too: naming past releases is its job.
if grep -rEn --exclude-dir=assets 'nxr [0-9]+\.[0-9]+\.[0-9]+' \
    README.md README.ru.md crates/nexus-raw/README.md docs; then
  echo "version-check: a page pins the current version" >&2
  failed=1
fi

[ "$failed" = 0 ] || exit 1
echo "version-check: $ws agrees everywhere"
