#!/usr/bin/env bash
# The CI verdict for the current commit: wait for the run, then judge it.
#
#   scripts/ci-check.sh          # HEAD, waits until the run settles
#   scripts/ci-check.sh --sha X  # any commit
#   scripts/ci-check.sh --quick  # report now, no waiting
#
# Exit code follows the verdict: 0 green, 1 red or no run found.
set -eu

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

# Find the run for this commit: the check run, not a nightly or a release.
run=""
i=0
if [ "$quick" -ne 1 ]; then
  printf '%sci-check:%s %s: looking for the CI run%s\n' "$DIM" "$N" "$short" "$N"
fi
while [ "$i" -lt 30 ]; do
  run="$(gh run list --workflow CI --commit "$sha" --limit 1 --json databaseId,status --jq '.[0].databaseId' 2>/dev/null || true)"
  [ -n "$run" ] && break
  i=$((i + 1))
  sleep 2
done
if [ -z "$run" ]; then
  echo "ci-check: no CI run found for $short" >&2
  exit 1
fi

if [ "$quick" -eq 1 ]; then
  out="$(gh run view "$run" --json status,conclusion 2>/dev/null || true)"
  status="$(jq -r '.status' <<<"$out")"
  case "$status" in
    completed) ;;
    *) printf '%sci-check:%s %s is still %s (run %s)\n' "$DIM" "$N" "$short" "$status" "$run"
       exit 3 ;;
  esac
else
  # A run that already settled never re-appears in watch's poll loop on some
  # gh versions: judge the state first, watch only a run still in flight.
  out="$(gh run view "$run" --json status,conclusion 2>/dev/null || true)"
  if [ "$(jq -r '.status' <<<"$out")" != completed ]; then
    # Stream the progress: a silent ten-minute terminal reads as a hang.
    gh run watch "$run" --interval 10 --exit-status >/dev/null 2>&1 || true
    out="$(gh run view "$run" --json status,conclusion 2>/dev/null || true)"
    while [ "$(jq -r '.status' <<<"$out")" != completed ]; do
      printf '%sci-check:%s still waiting on run %s%s\n' "$DIM" "$N" "$run" "$N"
      sleep 10
      out="$(gh run view "$run" --json status,conclusion 2>/dev/null || true)"
    done
  fi
fi

conclusion="$(jq -r '.conclusion' <<<"$out")"
if [ "$conclusion" = success ]; then
  printf '%sci-check:%s %s is %sGREEN%s\n' "$DIM" "$N" "$short" "$G" "$N"
else
  printf '%sci-check:%s %s is %s%s%s (%s)\n' "$DIM" "$N" "$short" "$R" "${conclusion:-unknown}" "$N" "$run" >&2
  exit 1
fi
