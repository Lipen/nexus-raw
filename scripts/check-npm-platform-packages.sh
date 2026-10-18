#!/bin/sh
# The staged platform packages must be on the registry before the main
# package declares them as its optionalDependencies: a consumer of the
# version must never hit a missing addon.
#
#   check-npm-platform-packages.sh
#
# Polls each package through the npm registry with a 12-attempt,
# 30-second-apart budget and refuses the publish on the first one that never
# appears.
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
  # The registry serves fresh publishes with a lag, and the npm CLI caches a
  # missing packument negatively: one early 404 poisons every later npm view
  # for minutes (caught live in the 0.11.1 cycle). A plain curl to the
  # registry origin carries no such cache, so the poll reads the truth as
  # soon as it lands.
  ok=0
  for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
    if curl -sf "https://registry.npmjs.org/$(printf '%s' "$name" | sed 's|/|%2F|g')" | grep -q "\"$version\""; then
      ok=1
      break
    fi
    echo "$name@$version not visible yet (attempt $attempt), waiting"
    sleep 30
  done
  if [ "$ok" != 1 ]; then
    echo "refusing to publish @nexus-raw/nxr@$version: $name@$version is not on the registry, its optionalDependencies entry would fail to resolve" >&2
    exit 1
  fi
  echo "$name@$version is on the registry"
done
