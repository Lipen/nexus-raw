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
lock="$(sed -n 's/^  "version": "\(.*\)",/\1/p' crates/nexus-raw-napi/package-lock.json | head -1)"
dep="$(sed -n 's/.*nexus-raw-core = { path = "\.\.\/nexus-raw-core", version = "\(.*\)" }.*/\1/p' crates/nexus-raw/Cargo.toml)"

failed=0
for pair in "crates/nexus-raw-napi/package.json:$pkg" "crates/nexus-raw-napi/package-lock.json:$lock" "nexus-raw's dependency on nexus-raw-core:$dep"; do
  name="${pair%%:*}"; got="${pair#*:}"
  if [ "$got" != "$ws" ]; then
    echo "version-check: $name is '$got', the workspace is '$ws'" >&2
    failed=1
  fi
done

# Pages may name the command, never the current number: a pinned version is a
# lie on the next release. The recorded session under docs/assets is exempt —
# it names the binary that made the recording, and `just demo-cast` refreshes it.
if grep -rEn --exclude-dir=assets 'nxr [0-9]+\.[0-9]+\.[0-9]+' \
    README.md README.ru.md crates/nexus-raw/README.md docs; then
  echo "version-check: a page pins the current version" >&2
  failed=1
fi

[ "$failed" = 0 ] || exit 1
echo "version-check: $ws agrees everywhere"
