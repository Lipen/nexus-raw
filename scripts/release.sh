#!/usr/bin/env bash
# The whole release act: the preview by default, `--yes` for the real run.
#
#   scripts/release.sh 0.12.0          everything the act would touch and
#                                      every gate it must pass, drawn first
#   scripts/release.sh 0.12.0 --yes    bump, changelog, push, rehearse, dispatch, watch
#
# The preview is read-only: the remote checks are GETs, the tree is not
# modified, nothing is pushed or published.
# The act rehearses before it dispatches: the same workflow on the same
# commit with dry-run=true, nothing published and no tag. A workflow that
# cannot run fails there, while the version number is still unspent.
# A repeat --yes call for the same number is the recovery path: the version
# step is idempotent, the changelog date refreshes to today, the guards in
# the workflow skip whatever is already published.
set -eu

usage() {
  if [ "${1:-}" = "0" ]; then
    echo "usage: release.sh X.Y.Z [--yes]"
    echo "  bumps the workspace, finalizes the changelog, pushes, rehearses and dispatches the release"
  else
    echo "usage: release.sh X.Y.Z [--yes]" >&2
    echo "  bumps the workspace, finalizes the changelog, pushes, rehearses and dispatches the release" >&2
  fi
  exit "${1:-2}"
}
case "${1:-}" in
  -h|--help) usage 0 ;;
esac
[ "$#" -ge 1 ] || usage
v="$1"; shift
if ! grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+' <<<"$v"; then
  echo "release: $v is not X.Y.Z" >&2
  exit 2
fi
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

# An info row: a dim 9-char label, then the payload.
row() { printf '%s%-9s%s %s\n' "$DIM" "$1" "$N" "$2"; }
# A continuation line under a row.
cont() { printf '%s%9s%s %s\n' "" "" "$N" "$1"; }

# The id of the run a dispatch just created on a commit, left in `run`.
# The dispatch API is asynchronous: the run only appears in the list a few
# seconds later, so the poll retries. `skip` is a run id to ignore, which
# keeps the real dispatch's poll off the rehearsal run it follows.
find_run() { # find_run <sha> <skip>
  for _ in 1 2 3 4 5 6; do
    sleep 5
    run="$(gh run list --workflow=Release --commit "$1" --limit 20 --json databaseId \
      --jq "[.[] | select(.databaseId != ${2:-0})][0].databaseId" 2>/dev/null || true)"
    case "$run" in '' | null) run="" ;; *) return 0 ;; esac
  done
  return 1
}

# Stream a run to its conclusion and leave the verdict in `conclusion`.
# The watch streams progress and dies quietly on a network hiccup; the loop
# below owns the verdict. It polls the run state, not the watch exit code:
# a failed run exits nonzero too, and retrying that would spin forever.
settle() { # settle <run id>
  echo "watching https://github.com/Lipen/nexus-raw/actions/runs/$1 until it settles (progress streams below)"
  gh run watch "$1" --interval 30 > /dev/null 2>&1 || true
  while :; do
    if out="$(gh run view "$1" --json status,conclusion 2>/dev/null)"; then
      status="$(jq -r '.status' <<<"$out")"
      case "$status" in
        completed) break ;;
        *) sleep 30 ;;
      esac
    else
      sleep 10
    fi
  done
  conclusion="$(jq -r '.conclusion' <<<"$out")"
}

blockers=0
notes=0

# ---- what moves --------------------------------------------------------
printf '%srelease %s%s %s· preview, nothing is pushed or published%s\n\n' "$B" "$v" "$N" "$DIM" "$N"

old="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$old" ] || { echo "release: no workspace version in Cargo.toml" >&2; exit 1; }
row version "${DIM}$old${N} ${G}→${N} ${B}$v${N}"
cont "${DIM}Cargo.toml · napi package.json · 2 dep pins · 3 lockfiles${N}"
echo

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
  row changelog "${Y}[Unreleased] is empty${N}"
  notes=$((notes + 1))
else
  row changelog "${DIM}[Unreleased]${N} → ${B}[$v]${N} ${DIM}- $today${N}"
  cont "${DIM}$parts${N}"
fi

# ---- the gates ---------------------------------------------------------
# Green collapses to one line; a note or a block unfolds every row.
gtree=0; gtag=0; gcrates=0; gpins=0
gtree_msg="clean"
gtag_msg=""
gcrates_msg=""
gpins_msg=""

[ -z "$(git status --porcelain)" ] || {
  n="$(git status --porcelain | wc -l | tr -d ' ')"
  gtree=2
  [ "$n" -eq 1 ] && gtree_msg="1 changed path, --yes refuses" || gtree_msg="$n changed paths, --yes refuses"
  blockers=$((blockers + 1))
}

rc=0
git ls-remote --exit-code origin "refs/tags/v$v" >/dev/null 2>&1 || rc=$?
case "$rc" in
  0) gtag=2; gtag_msg="v$v already exists on origin"; blockers=$((blockers + 1)) ;;
  2) gtag=0; gtag_msg="v$v is free" ;;
  *) gtag=1; gtag_msg="origin unreachable, skipped"; notes=$((notes + 1)) ;;
esac

if body="$(curl -sf "https://index.crates.io/ne/xu/nexus-raw-core" 2>/dev/null)"; then
  if grep -q "\"vers\":\"$v\"" <<<"$body"; then
    gcrates=1; gcrates_msg="$v already published, the crates job skips"; notes=$((notes + 1))
  else
    gcrates=0; gcrates_msg="$v is free"
  fi
else
  gcrates=1; gcrates_msg="crates.io unreachable, skipped"; notes=$((notes + 1))
fi

if scripts/version-check.sh >/dev/null 2>&1; then
  gpins=0; gpins_msg="the version agrees everywhere"
else
  gpins=2; gpins_msg="version-check fails"; blockers=$((blockers + 1))
fi

echo
if [ "$blockers" -eq 0 ] && [ "$notes" -eq 0 ]; then
  row checks "${G}all clear${N} ${DIM}· tree · tag · crates · pins${N}"
else
  first=1
  for gate in "tree:$gtree:$gtree_msg" "tag:$gtag:$gtag_msg" "crates:$gcrates:$gcrates_msg" "pins:$gpins:$gpins_msg"; do
    name="${gate%%:*}"; rest="${gate#*:}"; st="${rest%%:*}"; msg="${rest#*:}"
    case "$st" in
      0) m="${G}   ok${N}" ;;
      1) m="${Y} note${N}" ;;
      2) m="${R}block${N}" ;;
    esac
    if [ "$first" -eq 1 ]; then
      printf '%s%-9s%s %s %s %s%s\n' "$DIM" checks "$N" "$m" "$name" "$msg" "$N"
      first=0
    else
      printf '%9s %s %s %s%s\n' "" "$m" "$name" "$msg" "$N"
    fi
  done
fi

# ---- what runs ----------------------------------------------------------
echo
row pipeline "gate ${G}→${N} builds ${DIM}×5${N} ${G}→${N} crates ${DIM}×4${N} ${G}→${N} addons ${DIM}×5${N} ${G}→${N} npm ${DIM}×5${N} ${G}→${N} tag + release"
cont "${DIM}linux-x64 · linux-aarch64 · darwin-x64 · darwin-arm64 · windows-x64${N}"
row rehearsal "${DIM}the same pipeline ×2${N} ${G}→${N} dry-run first, the release second"
cont "${DIM}--yes runs it on the bumped commit with dry-run=true, nothing published, no tag${N}"
cont "${DIM}and dispatches the real run only once the rehearsal run is green${N}"

# ---- verdict ------------------------------------------------------------
echo
if [ "$blockers" -eq 0 ]; then
  row next "${B}${G}just release $v --yes${N}"
elif [ "$blockers" -eq 1 ]; then
  row next "${R}resolve the block above, then rerun${N}"
else
  row next "${R}resolve the $blockers blocks above, then rerun${N}"
fi

# ---- the act ------------------------------------------------------------
[ -n "$yes" ] || exit 0

echo
printf '%srelease %s%s %s· the real act%s\n\n' "$B" "$v" "$N" "$DIM" "$N"

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

# ---- the rehearsal ------------------------------------------------------
# The same workflow on the same commit with `dry-run=true`: the gate, every
# build and the staged npm install run, nothing is published and no tag is
# created. A workflow that cannot run fails here, while the version number
# is still unspent.
echo "rehearsing the release on $sha"
gh workflow run release.yml --ref master -F dry-run=true
find_run "$sha" "" || { echo "release: the rehearsal run did not appear" >&2; exit 1; }
rehearsal="$run"
echo "the rehearsal appeared: https://github.com/Lipen/nexus-raw/actions/runs/$rehearsal"
settle "$rehearsal"
if [ "$conclusion" != success ]; then
  echo "release: the rehearsal ended $conclusion, nothing was dispatched, see https://github.com/Lipen/nexus-raw/actions/runs/$rehearsal" >&2
  exit 1
fi

# ---- the release --------------------------------------------------------
echo "dispatching the release on $sha"
gh workflow run release.yml --ref master
find_run "$sha" "$rehearsal" || { echo "release: the dispatched run did not appear" >&2; exit 1; }
echo "the run appeared: https://github.com/Lipen/nexus-raw/actions/runs/$run"
settle "$run"
if [ "$conclusion" != success ]; then
  echo "release: the run ended $conclusion, see https://github.com/Lipen/nexus-raw/actions/runs/$run" >&2
  exit 1
fi
echo "released: https://github.com/Lipen/nexus-raw/releases/tag/v$v"
