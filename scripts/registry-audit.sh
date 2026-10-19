#!/bin/sh
# The finalize audit: every artifact of the release is visible on its
# registry. The tag only lands on a whole release, and the 0.2.0 release
# once shipped green with a package missing from the registry.
#
#   registry-audit.sh
#
# The crates index, npm and PyPI are probed for the workspace version
# through scripts/registry-probe.sh, each with a 20-attempt,
# 30-second-apart budget.
# The npm document can trail its publish by ten minutes, the same lag that
# held the 0.22.0 platform package.
# A miss is recorded and the audit continues, so one pass names every
# missing artifact, and the exit code judges the whole set.
#
# Exit codes: 0 every artifact is visible, 1 something is missing, 2 bad input.
set -eu

usage() {
  echo "usage: registry-audit.sh"
  echo "  asserts every artifact of the workspace version is on its registry"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

here="$(dirname "$0")"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$version" ] || { echo "registry-audit: no workspace version in Cargo.toml" >&2; exit 1; }

attempts=20
interval=30
fail=0
for crate in nexus-raw-core nexus-raw nexus-raw-tui mock-nexus; do
  "$here/registry-probe.sh" crates "$crate" "$version" "$attempts" "$interval" || fail=1
done
# The main package, its platform packages, then the bridge last: the same
# list the npm job publishes.
for name in \
  @nexus-raw/nxr \
  @nexus-raw/nxr-linux-x64-gnu \
  @nexus-raw/nxr-linux-arm64-gnu \
  @nexus-raw/nxr-darwin-x64 \
  @nexus-raw/nxr-darwin-arm64 \
  @nexus-raw/nxr-win32-x64-msvc
do
  "$here/registry-probe.sh" npm "$name" "$version" "$attempts" "$interval" || fail=1
done
"$here/registry-probe.sh" pypi nexus-raw "$version" "$attempts" "$interval" || fail=1
"$here/registry-probe.sh" npm nexus-raw "$version" "$attempts" "$interval" || fail=1
[ "$fail" = 0 ] || exit 1
echo "every artifact of $version is live"
