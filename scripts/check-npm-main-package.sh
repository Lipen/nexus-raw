#!/bin/sh
# The bridge depends on the main package at the exact same version: an
# install of the bridge before main lands fails ERESOLVE. The same rule the
# npm job holds for its platform packages, one level down.
#
#   check-npm-main-package.sh
#
# Polls @nexus-raw/nxr at the bridge's version through the npm registry with
# a 12-attempt, 30-second-apart budget and refuses to let the bridge publish
# without it.
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

version="$(sed -n 's/^  "version": "\(.*\)",/\1/p' packaging/npm-bridge/package.json)"
[ -n "$version" ] || { echo "check-npm-main-package: no version in packaging/npm-bridge/package.json" >&2; exit 1; }

# The npm CLI caches a missing packument negatively, so the poll reads the
# registry origin directly.
ok=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
  if curl -sf "https://registry.npmjs.org/@nexus-raw%2Fnxr" | grep -q "\"$version\""; then
    ok=1
    break
  fi
  echo "@nexus-raw/nxr@$version not visible yet (attempt $attempt), waiting"
  sleep 30
done
[ "$ok" = 1 ] || { echo "refusing to publish the bridge: @nexus-raw/nxr@$version is not on the registry, its dependency would fail to resolve" >&2; exit 1; }
echo "@nexus-raw/nxr@$version is on the registry"
