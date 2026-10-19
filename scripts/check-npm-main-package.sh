#!/bin/sh
# The bridge depends on the main package at the exact same version: an
# install of the bridge before main lands fails ERESOLVE. The same rule the
# npm job holds for its platform packages, one level down.
#
#   check-npm-main-package.sh
#
# Polls @nexus-raw/nxr at the bridge's version through
# scripts/registry-probe.sh with a 20-attempt, 30-second-apart budget and
# refuses to let the bridge publish without it.
# The npm document can trail its publish by ten minutes, the same lag that
# held the 0.22.0 platform package.
#
# Exit codes: 0 the main package is on the registry, 1 it is not, 2 bad input.
set -eu

usage() {
  echo "usage: check-npm-main-package.sh"
  echo "  asserts @nexus-raw/nxr at the bridge's version is on the npm registry"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

here="$(dirname "$0")"
version="$(sed -n 's/^  "version": "\(.*\)",/\1/p' packaging/npm-bridge/package.json)"
[ -n "$version" ] || { echo "check-npm-main-package: no version in packaging/npm-bridge/package.json" >&2; exit 1; }

"$here/registry-probe.sh" npm @nexus-raw/nxr "$version" 20 30 || {
  echo "refusing to publish the bridge: @nexus-raw/nxr@$version is not on the registry, its dependency would fail to resolve" >&2
  exit 1
}
echo "@nexus-raw/nxr@$version is on the registry"
