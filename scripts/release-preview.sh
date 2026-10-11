#!/usr/bin/env bash
# The release preview: everything a real run would touch and every gate it
# must pass, drawn before the act.
# Read-only: the remote checks are GETs, the tree is not modified, nothing
# is pushed or published.
set -eu

v="${1:?usage: release-preview.sh X.Y.Z}"

if [ -t 1 ]; then
  B=$'\033[1m' DIM=$'\033[2m' G=$'\033[32m' Y=$'\033[33m' R=$'\033[31m' N=$'\033[0m'
else
  B="" DIM="" G="" Y="" R="" N=""
fi

# One row: a 9-char dim label, then the payload.
row() { printf '%s%-10s%s%s\n' "$DIM" "$1" "$N" "$2"; }
# A row with a status mark: 0 ok, 1 note, 2 block.
mark() {
  local m
  case "$2" in
    0) m="${G}  ok${N}" ;;
    1) m="${Y} note${N}" ;;
    2) m="${R}block${N}" ;;
    *) m="    " ;;
  esac
  printf '%s%-10s%s%s %s\n' "$DIM" "$1" "$N" "$m" "$3"
}

blockers=0

printf '%srelease %s%s %spreview: nothing is pushed or published%s\n\n' "$B" "$v" "$N" "$DIM" "$N"

# ---- version ----------------------------------------------------------
old="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$old" ] || { echo "release: no workspace version in Cargo.toml" >&2; exit 1; }
if [ "$old" = "$v" ]; then
  row version "already $v, the bump is a no-op"
else
  row version "$old ${G}->${N} ${B}$v${N}"
  row "" "${DIM}Cargo.toml · napi package.json · 2 dep pins · 3 lockfiles (examples/node follows the release)${N}"
fi

# ---- changelog ---------------------------------------------------------
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

# ---- gates -------------------------------------------------------------
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
  *) mark tag 1 "origin unreachable, the tag check is skipped" ;;
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

# ---- what a --yes run would do ------------------------------------------
row pipeline "gate ${G}->${N} builds ${DIM}x5${N} ${G}->${N} crates ${DIM}x4${N} ${G}->${N} addons ${DIM}x5${N} ${G}->${N} npm ${DIM}x5${N} ${G}->${N} tag + release"
row "" "${DIM}linux-x64 · linux-aarch64 · darwin-x64 · darwin-arm64 · windows-x64${N}"

# ---- verdict ------------------------------------------------------------
echo
if [ "$blockers" -eq 0 ]; then
  row next "${B}${G}just release $v --yes${N}"
else
  row next "resolve the ${R}blocks${N} above, then rerun"
fi
echo
