#!/bin/sh
# The release gate: the full CI matrix on this commit must be green before
# the release workflow publishes anything.
# The bump push starts CI on the same commit.
# A manual dispatch on a later day checks that same run, so the release does
# not depend on the terminal that pushed the bump.
#
#   check-ci-run.sh <sha> [attempts [interval]]
#
# The run can be queued or in progress when the release gate starts, so the
# check waits up to the budget.
# A missing run is a failure: the release must not ride a commit the matrix
# never saw.
#
# Exit codes: 0 the CI run succeeded, 1 missing or not green, 2 bad input.
set -eu

usage() {
  echo "usage: check-ci-run.sh <sha> [attempts [interval]]"
  echo "  waits for the CI run on <sha> to succeed"
  echo "  attempts default to 80, interval to 15 seconds"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -ge 1 ] && [ "$#" -le 3 ] || usage_err
sha="$1"
attempts="${2:-80}"
interval="${3:-15}"
case "$sha" in ''|*[!0-9a-fA-F]*) usage_err ;; esac
case "$attempts" in ''|*[!0-9]*) usage_err ;; esac
case "$interval" in ''|*[!0-9]*) usage_err ;; esac
[ "$attempts" -ge 1 ] || usage_err

# The push and the release dispatch race: the run may not be listed yet.
run=""
n=1
while [ "$n" -le 6 ]; do
  run="$(gh run list --workflow=CI --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId // empty' 2>/dev/null || true)"
  [ -n "$run" ] && break
  [ "$n" -lt 6 ] && sleep 5
  n=$((n + 1))
done
if [ -z "$run" ]; then
  echo "check-ci-run: no CI run found for $sha, the matrix never saw this commit" >&2
  exit 1
fi

n=1
url=""
while :; do
  line="$(gh run view "$run" --json status,conclusion,url --jq '[.status, .conclusion, (.url // "")] | @tsv' 2>/dev/null || true)"
  if [ -n "$line" ]; then
    status="$(printf '%s' "$line" | cut -f1)"
    conclusion="$(printf '%s' "$line" | cut -f2)"
    url="$(printf '%s' "$line" | cut -f3)"
    if [ "$status" = completed ]; then
      if [ "$conclusion" = success ]; then
        echo "check-ci-run: CI on $sha is green${url:+ ($url)}"
        exit 0
      fi
      echo "check-ci-run: CI on $sha ended $conclusion${url:+, see $url}" >&2
      exit 1
    fi
  fi
  if [ "$n" -ge "$attempts" ]; then
    echo "check-ci-run: CI on $sha did not settle in $((attempts * interval))s${url:+, last seen $url}" >&2
    exit 1
  fi
  sleep "$interval"
  n=$((n + 1))
done
