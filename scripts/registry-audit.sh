#!/bin/sh
# The finalize audit: every artifact of the release is visible on its
# registry. The tag only lands on a whole release, and the 0.2.0 release
# once shipped green with a package missing from the registry.
#
#   registry-audit.sh
#
# The crates index, npm and PyPI are probed for the workspace version, each
# with a 12-attempt, 30-second-apart budget. A miss is recorded and the
# audit continues, so one pass names every missing artifact; the exit code
# judges the whole set.
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

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$version" ] || { echo "registry-audit: no workspace version in Cargo.toml" >&2; exit 1; }

# The crates.io index nests by name length: 1/<c>, 2/<cc>, 3/<c>/<ccc>, and
# <first two>/<next two>/<name> from four characters up.
crates_path() { # crates_path <name>
  case "${#1}" in
    1) printf '1/%s' "$1" ;;
    2) printf '2/%s' "$1" ;;
    3) printf '3/%s/%s' "$(printf '%s' "$1" | cut -c1)" "$1" ;;
    *) printf '%s/%s/%s' "$(printf '%s' "$1" | cut -c1-2)" "$(printf '%s' "$1" | cut -c3-4)" "$1" ;;
  esac
}

fail=0
for c in nexus-raw-core nexus-raw nexus-raw-tui; do
  ok=0
  for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
    if curl -sf "https://index.crates.io/$(crates_path "$c")" | grep -q "\"vers\":\"$version\""; then
      ok=1
      break
    fi
    echo "$c@$version not visible yet (attempt $attempt), waiting"
    sleep 30
  done
  [ "$ok" = 1 ] || { echo "$c@$version is not on crates.io" >&2; fail=1; }
done
ok=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
  if curl -sf "https://index.crates.io/$(crates_path mock-nexus)" | grep -q "\"vers\":\"$version\""; then
    ok=1
    break
  fi
  echo "mock-nexus@$version not visible yet (attempt $attempt), waiting"
  sleep 30
done
[ "$ok" = 1 ] || { echo "mock-nexus@$version is not on crates.io" >&2; fail=1; }
for name in @nexus-raw/nxr @nexus-raw/nxr-linux-x64-gnu @nexus-raw/nxr-linux-arm64-gnu @nexus-raw/nxr-darwin-x64 @nexus-raw/nxr-darwin-arm64 @nexus-raw/nxr-win32-x64-msvc; do
  ok=0
  # The version document, not the packument `npm view` reads: the packument
  # rides the registry CDN and the CLI caches a miss negatively, so a fresh
  # publish can take minutes to show there while `/name/version` appears
  # with the publish itself.
  for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
    if curl -sf "https://registry.npmjs.org/$(printf '%s' "$name" | sed 's|/|%2F|g')/$version" > /dev/null 2>&1; then
      ok=1
      break
    fi
    echo "$name@$version not visible yet (attempt $attempt), waiting"
    sleep 30
  done
  [ "$ok" = 1 ] || { echo "$name@$version is not on npm" >&2; fail=1; }
done
ok=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
  # The PyPI JSON serves `"version": "…"` with a space: the optional one
  # keeps the check honest whatever the serializer chooses.
  if curl -sf "https://pypi.org/pypi/nexus-raw/json" | grep -Eq "\"version\": ?\"$version\""; then
    ok=1
    break
  fi
  echo "nexus-raw@$version not visible on PyPI yet (attempt $attempt), waiting"
  sleep 30
done
[ "$ok" = 1 ] || { echo "nexus-raw@$version is not on PyPI" >&2; fail=1; }
ok=0
for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
  if curl -sf "https://registry.npmjs.org/nexus-raw/$version" > /dev/null 2>&1; then
    ok=1
    break
  fi
  echo "the npm bridge nexus-raw@$version not visible yet (attempt $attempt), waiting"
  sleep 30
done
[ "$ok" = 1 ] || { echo "the npm bridge nexus-raw@$version is not on npm" >&2; fail=1; }
[ "$fail" = 0 ] || exit 1
echo "every artifact of $version is live"
