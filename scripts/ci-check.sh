#!/usr/bin/env bash
# The CI verdict for a commit: poll the run state until it settles, then judge.
#
#   scripts/ci-check.sh                # HEAD, waits for the verdict
#   scripts/ci-check.sh --sha X        # any commit
#   scripts/ci-check.sh --quick        # report the current state, no waiting
#
# Exit codes: 0 green, 1 red or no run, 2 bad input, 3 still running (--quick).
set -eu

# Seconds between state polls. The loop checks the real state every tick,
# so this is a courtesy to the API, not a guess about timing.
POLL_SECS=5

sha="$(git rev-parse --verify --quiet HEAD^{commit} 2>/dev/null || true)"
if [ -z "$sha" ]; then
  echo "ci-check: not a git repository or no HEAD commit" >&2
  exit 2
fi
quick=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --sha)
      [ -n "${2:-}" ] || { echo "usage: ci-check.sh [--sha X] [--quick]" >&2; exit 2; }
      if ! sha="$(git rev-parse --verify --quiet "$2^{commit}" 2>/dev/null)"; then
        echo "ci-check: unknown commit '$2'" >&2
        exit 2
      fi
      shift 2
      ;;
    --quick) quick=1; shift ;;
    *) echo "usage: ci-check.sh [--sha X] [--quick]" >&2; exit 2 ;;
  esac
done
short="$(git rev-parse --short "$sha")"

if [ -t 1 ]; then
  G=$'\033[32m' R=$'\033[31m' DIM=$'\033[2m' N=$'\033[0m'
else
  G="" R="" DIM="" N=""
fi

# Find the check run of this commit: not a nightly, not a release.
run=""
while :; do
  run="$(gh run list --workflow CI --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
  if [ -n "$run" ]; then break; fi
  if [ "$quick" -eq 1 ]; then
    echo "ci-check: no CI run found for $short" >&2
    exit 1
  fi
  printf '%sci-check:%s %s: waiting for the run to appear%s\n' "$DIM" "$N" "$short" "$N"
  sleep "$POLL_SECS"
done

# One loop owns the wait: read the state, report transitions, stop when settled.
last=""
out=""
while :; do
  if out="$(gh run view "$run" --json status,conclusion 2>/dev/null)"; then
    status="$(jq -r '.status' <<<"$out")"
    if [ "$status" = completed ]; then break; fi
    if [ "$status" != "$last" ]; then
      printf '%sci-check:%s %s is %s (run %s)%s\n' "$DIM" "$N" "$short" "$status" "$run" "$N"
      last="$status"
    fi
  fi
  if [ "$quick" -eq 1 ]; then
    printf '%sci-check:%s %s is %s (run %s)%s\n' "$DIM" "$N" "$short" "$status" "$run" "$N" >&2
    exit 3
  fi
  sleep "$POLL_SECS"
done

conclusion="$(jq -r '.conclusion' <<<"$out")"
if [ "$conclusion" = success ]; then
  printf '%sci-check:%s %s is %sGREEN%s\n' "$DIM" "$N" "$short" "$G" "$N"
else
  printf '%sci-check:%s %s is %s%s%s (run %s)%s\n' "$DIM" "$N" "$short" "$R" "${conclusion:-unknown}" "$run" "$N" >&2
  exit 1
fi
