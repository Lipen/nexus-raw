#!/bin/sh
# One place that asks a registry whether a name carries a version:
# crates.io, npm and PyPI all answer through here.
#
#   registry-probe.sh <crates|npm|pypi> <name> <version> [attempts [interval]]
#
# A single attempt is a predicate: the answer is the exit code and nothing is
# printed, so the caller owns its verdict line. More attempts make it a poll:
# every failed attempt prints one line and the verdict prints one line.
#
# The npm question reads the version document
# (https://registry.npmjs.org/<name>/<version>), never `npm view`: the
# packument rides the registry CDN and the npm CLI caches a miss negatively,
# so one early 404 poisons every later view for minutes (caught live in the
# 0.11.1 cycle), while the version document appears with the publish itself.
# The crates question reads the index file, PyPI the JSON document.
#
# Exit codes: 0 visible, 1 not visible, 2 bad input.
set -eu

usage() {
  echo "usage: registry-probe.sh <crates|npm|pypi> <name> <version> [attempts [interval]]"
  echo "  asks the registry whether <name>@<version> is served"
  echo "  one attempt answers through the exit code, more attempts report every decision"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
if [ "$#" -lt 3 ] || [ "$#" -gt 5 ]; then usage_err; fi

registry="$1"
name="$2"
version="$3"
attempts="${4:-1}"
interval="${5:-30}"
case "$registry" in
  crates|npm|pypi) ;;
  *) usage_err ;;
esac
case "$attempts" in
  ''|*[!0-9]*) usage_err ;;
esac
case "$interval" in
  ''|*[!0-9]*) usage_err ;;
esac
[ "$attempts" -ge 1 ] || usage_err

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

visible() { # visible <name> <version>
  case "$registry" in
    crates)
      curl -sf "https://index.crates.io/$(crates_path "$1")" | grep -q "\"vers\":\"$2\""
      ;;
    npm)
      # A scoped name travels as @scope%2Fname in the path.
      curl -sf "https://registry.npmjs.org/$(printf '%s' "$1" | sed 's|/|%2F|g')/$2" > /dev/null 2>&1
      ;;
    pypi)
      # The PyPI JSON serves `"version": "…"` with a space: the optional one
      # keeps the check honest whatever the serializer chooses.
      curl -sf "https://pypi.org/pypi/$1/json" | grep -Eq "\"version\": ?\"$2\""
      ;;
  esac
}

case "$registry" in
  crates) where="crates.io" ;;
  npm) where="npm" ;;
  pypi) where="PyPI" ;;
esac

attempt=1
while [ "$attempt" -le "$attempts" ]; do
  if visible "$name" "$version"; then
    [ "$attempts" -eq 1 ] || echo "$name@$version is on $where"
    exit 0
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    echo "$name@$version not visible yet (attempt $attempt), waiting"
    sleep "$interval"
  fi
  attempt=$((attempt + 1))
done
[ "$attempts" -eq 1 ] || echo "$name@$version is not on $where" >&2
exit 1
