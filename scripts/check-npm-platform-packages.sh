#!/bin/sh
# The staged platform packages must be on the registry before the main
# package declares them as its optionalDependencies: a consumer of the
# version must never hit a missing addon.
#
#   check-npm-platform-packages.sh
#
# Polls each package through scripts/registry-probe.sh with a 20-attempt,
# 30-second-apart budget and refuses the publish on the first one that never
# appears.
# A platform document can trail its publish by ten minutes on a slow npm
# evening: the 0.22.0 arm64 package was still invisible when the old budget
# ran out, and the rerun found it on the registry minutes later.
#
# Exit codes: 0 every package is on the registry, 1 one is missing, 2 bad input.
set -eu

usage() {
  echo "usage: check-npm-platform-packages.sh"
  echo "  asserts the five staged platform packages are on the npm registry"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

here="$(dirname "$0")"
version="$(sed -n 's/^  "version": "\(.*\)",/\1/p' crates/nexus-raw-napi/package.json)"
[ -n "$version" ] || { echo "check-npm-platform-packages: no version in crates/nexus-raw-napi/package.json" >&2; exit 1; }

# The same list the stage-main step declares as optionalDependencies.
for name in \
  @nexus-raw/nxr-linux-x64-gnu \
  @nexus-raw/nxr-linux-arm64-gnu \
  @nexus-raw/nxr-darwin-x64 \
  @nexus-raw/nxr-darwin-arm64 \
  @nexus-raw/nxr-win32-x64-msvc
do
  "$here/registry-probe.sh" npm "$name" "$version" 20 30 || {
    echo "refusing to publish @nexus-raw/nxr@$version: $name@$version is not on the registry, its optionalDependencies entry would fail to resolve" >&2
    exit 1
  }
done
echo "the platform packages of $version are on the registry"
