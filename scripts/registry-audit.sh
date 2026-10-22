#!/bin/sh
# The finalize audit: every artifact of the release is visible on its
# registry. The tag only lands on a whole release, and the 0.2.0 release
# once shipped green with a package missing from the registry.
#
#   registry-audit.sh
#
# The crates index, npm and PyPI are probed for the workspace version
# through scripts/registry-probe.sh.
# One round probes every artifact once.
# A round that finds a miss sleeps and retries the whole set until a single
# deadline expires, so a slow registry cannot stretch the wait per package
# and the finalize job's timeout stays a hard ceiling.
# The npm document can trail its publish by more than ten minutes, the lag
# that held the 0.22.0 and 0.23.0 platform packages.
# A miss is recorded and the audit continues, so one round names every
# missing artifact, and the exit code judges the whole set.
#
# REGISTRY_AUDIT_DEADLINE and REGISTRY_AUDIT_INTERVAL override the default
# 900 seconds and 15 seconds.
# REGISTRY_AUDIT_VERSION overrides the workspace version, for a manual probe.
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
if [ -n "${REGISTRY_AUDIT_VERSION:-}" ]; then
  version="$REGISTRY_AUDIT_VERSION"
else
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
fi
[ -n "$version" ] || { echo "registry-audit: no workspace version in Cargo.toml" >&2; exit 1; }

deadline="${REGISTRY_AUDIT_DEADLINE:-900}"
interval="${REGISTRY_AUDIT_INTERVAL:-15}"
case "$deadline" in ''|*[!0-9]*) usage_err ;; esac
case "$interval" in ''|*[!0-9]*) usage_err ;; esac

probe() { # probe <registry> <name> <label>
  if "$here/registry-probe.sh" "$1" "$2" "$version" 1 0 >/dev/null 2>&1; then
    return 0
  fi
  echo "$2@$version is not visible on $3"
  return 1
}

check_all() {
  fail=0
  for crate in nexus-raw-core nexus-raw nexus-raw-tui mock-nexus; do
    probe crates "$crate" crates.io || fail=1
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
    probe npm "$name" npm || fail=1
  done
  probe npm nexus-raw npm || fail=1
  probe pypi nexus-raw PyPI || fail=1
  return "$fail"
}

start="$(date +%s)"
while :; do
  if check_all; then
    echo "every artifact of $version is live"
    exit 0
  fi
  now="$(date +%s)"
  [ $((now - start)) -ge "$deadline" ] && break
  sleep "$interval"
done
echo "registry-audit: $version is still incomplete after ${deadline}s" >&2
exit 1
