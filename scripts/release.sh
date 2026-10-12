#!/usr/bin/env bash
# The whole release act: the preview by default, `--yes` for the real run.
#
#   scripts/release.sh 0.12.0          everything the act would touch and
#                                      every gate it must pass, drawn first
#   scripts/release.sh 0.12.0 --yes    bump, changelog, push, dispatch, watch
#
# The preview is read-only: the remote checks are GETs, the tree is not
# modified, nothing is pushed or published.
# A repeat --yes call for the same number is the recovery path: the version
# step is idempotent, the changelog date refreshes to today, the guards in
# the workflow skip whatever is already published.
set -eu

usage() { echo "usage: release.sh X.Y.Z [--yes]" >&2; exit 2; }
[ "$#" -ge 1 ] || usage
v="$1"; shift
case "$v" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "release: $v is not X.Y.Z" >&2; exit 2 ;;
esac
yes=""
for a in "$@"; do
  case "$a" in
    --yes) yes=1 ;;
    *) usage ;;
  esac
done

if [ -t 1 ]; then
  B=$'\033[1m' DIM=$'\033[2m' G=$'\033[32m' Y=$'\033[33m' R=$'\033[31m' N=$'\033[0m'
else
  B="" DIM="" G="" Y="" R="" N=""
fi

# One row: a dim label, then the payload.
row() { printf '%s%-10s%s%s\n' "$DIM" "$1" "$N" "$2"; }
# A row with a status mark: 0 ok, 1 note, 2 block.
mark() {
  local m
  case "$2" in
    0) m="${G}  ok${N}" ;;
    1) m="${Y} note${N}" ;;
    2) m="${R}block${N}" ;;
  esac
  printf '%s%-10s%s%s %s\n' "$DIM" "$1" "$N" "$m" "$3"
}

blockers=0

# ---- preview -----------------------------------------------------------
# Every claim here mirrors what the act below really does.

printf '%srelease %s%s %spreview: nothing is pushed or published%s\n\n' "$B" "$v" "$N" "$DIM" "$N"

old="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$old" ] || { echo "release: no workspace version in Cargo.toml" >&2; exit 1; }
if [ "$old" = "$v" ]; then
  row version "already $v, the bump is a no-op"
else
  row version "$old ${G}->${N} ${B}$v${N}"
  row "" "${DIM}Cargo.toml · napi package.json · 2 dep pins · 3 lockfiles (examples/node follows the release)${N}"
fi

cl="$(python3 - "$v" <<'PY'
import datetime, pathlib, sys
t = pathlib.Path("CHANGELOG.md").read_text()
start = t.index("## [Unreleased]")
end = t.find("\n## [", start + 1)
block = t[start:end]
sections = []
for line in block.splitlines():
    if line.startswith("### "):
        sections.append([line[4:].strip(), 0])
    elif line.startswith("- ") and sections:
        sections[-1][1] += 1
today = datetime.date.today().isoformat()
parts = " · ".join(f"{name} {count}" for name, count in sections)
total = sum(count for _, count in sections)
print(f"{total}|{parts}|{today}")
PY
)"
total="$(cut -d'|' -f1 <<<"$cl")"
parts="$(cut -d'|' -f2 <<<"$cl")"
today="$(cut -d'|' -f3 <<<"$cl")"
if [ "$total" -eq 0 ]; then
  mark changelog 1 "[Unreleased] is empty: the notes would be empty"
else
  row changelog "[Unreleased] ${G}->${N} [$v] - $today"
  row "" "${DIM}$parts${N}"
fi

if [ -n "$(git status --porcelain)" ]; then
  n="$(git status --porcelain | wc -l | tr -d ' ')"
  mark tree 2 "dirty: $n changed paths, --yes refuses (commit or stash first)"
  blockers=$((blockers + 1))
else
  mark tree 0 "clean"
fi

rc=0
git ls-remote --exit-code origin "refs/tags/v$v" >/dev/null 2>&1 || rc=$?
case "$rc" in
  0) mark tag 2 "v$v exists on origin, the push would fail" ; blockers=$((blockers + 1)) ;;
  2) mark tag 0 "v$v is free" ;;
  *) mark tag 1 "origin unreachable, the check is skipped" ;;
esac

if body="$(curl -sf "https://index.crates.io/ne/xu/nexus-raw-core" 2>/dev/null)"; then
  if grep -q "\"vers\":\"$v\"" <<<"$body"; then
    mark crates 1 "$v is already published, the crates job skips it"
  else
    mark crates 0 "$v is free"
  fi
else
  mark crates 1 "crates.io unreachable, the check is skipped"
fi

if scripts/version-check.sh >/dev/null 2>&1; then
  mark pins 0 "the version agrees everywhere, no page pins a number"
else
  mark pins 2 "version-check fails, fix it before --yes"
  blockers=$((blockers + 1))
fi

row pipeline "gate ${G}->${N} builds ${DIM}x5${N} ${G}->${N} crates ${DIM}x4${N} ${G}->${N} addons ${DIM}x5${N} ${G}->${N} npm ${DIM}x5${N} ${G}->${N} tag + release"
row "" "${DIM}linux-x64 · linux-aarch64 · darwin-x64 · darwin-arm64 · windows-x64${N}"

echo
if [ "$blockers" -eq 0 ]; then
  row next "${B}${G}scripts/release.sh $v --yes${N}"
else
  row next "resolve the ${R}blocks${N} above, then rerun"
fi

# ---- the act ------------------------------------------------------------
[ -n "$yes" ] || exit 0

echo
printf '%srelease %s%s %sthe real act%s\n\n' "$B" "$v" "$N" "$DIM" "$N"

# The same gates the preview drew, enforced this time.
if [ -n "$(git status --porcelain)" ]; then
  echo "release $v: the tree is dirty, commit or stash first (a --yes run commits only its own bump)" >&2
  exit 1
fi

just version "$v"
python3 scripts/changelog-finalize.py "$v"
# Every lockfile follows the bump inside the same commit: the post-release
# `just lock` only refreshes examples/node against the now-published registry.
just lock
git add -u
if git diff --cached --quiet; then
  echo "release $v: the bump produced no changes, the version is already $v?" >&2
  exit 1
fi
git commit -m "chore: bump the workspace to $v"
git push origin master
sha="$(git rev-parse origin/master)"
# The dispatch API takes a branch, not a raw SHA: master is the ref, and
# the gate validates whatever HEAD it gets.
echo "dispatching the release on $sha"
gh workflow run release.yml --ref master
run=""
for i in 1 2 3 4 5 6; do
  sleep 5
  run="$(gh run list --workflow=Release --commit "$sha" --limit 1 --json databaseId --jq '.[0].databaseId' 2>/dev/null || true)"
  [ -n "$run" ] && break
done
[ -n "$run" ] || { echo "release: the dispatched run did not appear" >&2; exit 1; }
gh run watch "$run" --interval 30 --exit-status
echo "released: https://github.com/Lipen/nexus-raw/releases/tag/v$v"
