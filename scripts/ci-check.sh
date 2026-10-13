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

die() { echo "ci-check: $*" >&2; exit 2; }

sha="$(git rev-parse --verify --quiet HEAD^{commit} 2>/dev/null || true)"
[ -n "$sha" ] || die "not a git repository or no HEAD commit"

quick=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help)
      echo "usage: ci-check.sh [--sha X] [--quick]"
      echo "  polls the CI run for a commit until it settles; --quick takes the current state without waiting"
      exit 0
      ;;
    --sha)
      [ -n "${2:-}" ] || die "usage: ci-check.sh [--sha X] [--quick]"
      sha="$(git rev-parse --verify --quiet "$2^{commit}" 2>/dev/null)" \
          || die "unknown commit '$2'"
      shift 2
      ;;
    --quick) quick=1; shift ;;
    *) die "usage: ci-check.sh [--sha X] [--quick]" ;;
  esac
done
short="$(git rev-parse --short "$sha")"

if [ -t 1 ]; then
  G=$'\033[32m' R=$'\033[31m' DIM=$'\033[2m' N=$'\033[0m'
else
  G="" R="" DIM="" N=""
fi
say() { printf '%sci-check:%s %s\n' "$DIM" "$N" "$1"; }
verdict() { # $1 = conclusion, $2 = run id
  if [ "$1" = success ]; then
    printf '%sci-check:%s %s is %sGREEN%s\n' "$DIM" "$N" "$short" "$G" "$N"
    return 0
  fi
  printf '%sci-check:%s %s is %s%s%s (run %s)%s\n' "$DIM" "$N" "$short" "$R" "${1:-unknown}" "$N" "$2" >&2
  return 1
}

# Find the check run of this commit: not a nightly, not a release.
run=""
while :; do
  run="$(gh run list --workflow CI --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
  if [ -n "$run" ]; then break; fi
  if [ "$quick" -eq 1 ]; then
    echo "ci-check: no CI run found for $short" >&2
    exit 1
  fi
  say "$short: waiting for the run to appear"
  sleep "$POLL_SECS"
done

# One loop owns the wait: read the state, report transitions, tick between them.
# A tick every POLL_SECS keeps the terminal alive; a silent six-minute wait
# on a long CI reads as a hang.
last=""
out=""
ticks=0
while :; do
  if out="$(gh run view "$run" --json status,conclusion 2>/dev/null)"; then
    status="$(jq -r '.status' <<<"$out")"
    [ "$status" = completed ] && break
    if [ "$status" != "$last" ]; then
      say "$short is $status (run $run)"
      last="$status"
      ticks=0
    elif [ "$quick" -ne 1 ] && [ $((ticks % 6)) -eq 0 ]; then
      say "$short is still $status"
    fi
  fi
  [ "$quick" -eq 1 ] && { say "$short is $status (run $run)" >&2; exit 3; }
  ticks=$((ticks + 1))
  sleep "$POLL_SECS"
done

conclusion="$(jq -r '.conclusion' <<<"$out")"
verdict "$conclusion" "$run" || exit 1
