#!/bin/sh
# The agent skill must stay loadable and honest: a SKILL.md with a real
# frontmatter name and description, a name that matches its directory, and
# every relative link resolving to a file that exists.
set -eu

usage() {
  echo "usage: check-skill.sh"
  echo "  validates skills/nxr/SKILL.md frontmatter and its relative links"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

root="$(cd "$(dirname "$0")/.." && pwd)"
skill_dir="$root/skills/nxr"
skill="$skill_dir/SKILL.md"

fail() {
  echo "check-skill: $1" >&2
  exit 1
}

# Print one line per relative markdown link that points at nothing.
check_links() {
  grep -o ']([^)]*)' "$1" | sed 's/^](//; s/)$//' | while IFS= read -r target; do
    case "$target" in
      http://* | https://* | mailto:* | '#'*) continue ;;
    esac
    resolved="$(dirname "$1")/${target%%#*}"
    [ -e "$resolved" ] || echo "  $1 -> $target"
  done
}

[ -f "$skill" ] || fail "skills/nxr/SKILL.md is missing"

[ "$(head -n 1 "$skill")" = "---" ] || fail "SKILL.md must open with a frontmatter block (---)"

fm="$(awk 'NR == 1 { next } /^---[[:space:]]*$/ { exit } { print }' "$skill")"
name="$(printf '%s\n' "$fm" | sed -n 's/^name:[[:space:]]*//p')"
desc="$(printf '%s\n' "$fm" | sed -n 's/^description:[[:space:]]*//p')"

[ -n "$name" ] || fail "the frontmatter needs a name:"
[ -n "$desc" ] || fail "the frontmatter needs a description:"
[ "$name" = "nxr" ] || fail "the frontmatter name '$name' must match the directory 'nxr'"

# The description is the trigger. A wrapped one loses its tail in every loader
# that reads the value as a single line, and length bounds what a loader keeps.
desc_lines="$(printf '%s\n' "$fm" | grep -c '^description:' || true)"
[ "$desc_lines" -eq 1 ] || fail "the description must be a single line"

desc_len="$(printf '%s' "$desc" | wc -c | tr -d ' ')"
[ "$desc_len" -ge 40 ] || fail "the description is too short to trigger reliably ($desc_len chars)"
[ "$desc_len" -le 1024 ] || fail "the description is too long ($desc_len chars, loaders truncate past 1024)"

# A rename must not leave a dangling pointer the agent would follow into nothing.
dead="$(check_links "$skill")"
for f in "$skill_dir"/references/*.md; do
  [ -f "$f" ] || continue
  dead="$dead$(check_links "$f")"
done
[ -z "$dead" ] || fail "dead relative link(s):
$dead"

echo "check-skill: the nxr skill is well-formed ($desc_len-char description)"
