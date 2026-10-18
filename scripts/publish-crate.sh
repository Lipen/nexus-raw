#!/bin/sh
# Publish one workspace crate to crates.io, idempotent across re-runs.
#
#   publish-crate.sh <crate>
#
# The version is the workspace version in Cargo.toml.
# A crate already on the index is skipped, a crate that appears only after
# our failed attempt lost a race to a concurrent run (not a failure), and a
# crate that never appears fails the run: the three verdicts a re-dispatch
# after a half-done publish needs.
#
# Exit codes: 0 out or already out, 1 the publish failed, 2 bad input.
set -eu

usage() {
  echo "usage: publish-crate.sh <crate>"
  echo "  publishes <crate> at the workspace version, skipping one already on crates.io"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -eq 1 ] || usage_err
case "$1" in
  -*) usage_err ;;
esac

crate="$1"
here="$(dirname "$0")"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$version" ] || { echo "publish-crate: no workspace version in Cargo.toml" >&2; exit 1; }

if "$here/registry-probe.sh" crates "$crate" "$version"; then
  echo "$crate@$version is already on crates.io, skipping"
elif cargo publish --locked -p "$crate"; then
  echo "published"
elif "$here/registry-probe.sh" crates "$crate" "$version"; then
  echo "$crate@$version appeared after our publish attempt: lost the race to a concurrent run, not broken"
else
  echo "$crate@$version publish failed and the version is not visible" >&2
  exit 1
fi
