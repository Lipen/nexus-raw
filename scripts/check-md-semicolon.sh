#!/bin/bash
# Markdown prose must not splice sentences with `;`: the repo style is one
# sentence per line, sentences end with a period. Code blocks and inline code
# are exempt: a semicolon there is syntax, not typography.
set -eu

usage() {
  echo "usage: check-md-semicolon.sh"
  echo "  scans tracked markdown for semicolon-spliced sentences; exits 1 on a hit"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

hits=0
# Tracked markdown only: git ls-files keeps node_modules, target and site out
# by construction, and untracked scratch files never gate.
while IFS= read -r -d '' file; do
  # A tracked path is gone from the working tree until its deletion is staged:
  # `git ls-files` still lists it, and a missing file is not a hit.
  [ -f "$file" ] || continue
  # Strip fenced code blocks first, then look for the splice pattern:
  # a lowercase/uppercase word, `; `, and another word on the same line.
  # Inside a raw <figure> HTML block the semicolons are CSS or SVG syntax,
  # not typography: skip those lines too.
  # Inline code spans are syntax as well, so they are removed before the
  # pattern runs. Exempting whole lines that merely contained a semicolon
  # between two spans hid real splices, such as a table cell reading
  # "`ls` at any depth; `--assets` prints the names".
  matches=$(awk '
    /^```/ { fence = !fence; next }
    /<figure>/ { html = 1 }
    /<\/figure>/ { html = 0 }
    !fence && !html {
      line = $0
      gsub(/`[^`]*`/, "", line)
      if (line ~ /; /) print FILENAME ":" FNR ": " $0
    }
  ' "$file")
  if [ -n "$matches" ]; then
    echo "$matches" >&2
    hits=1
  fi
done < <(git ls-files -z -- '*.md' ':!:*.dev/*')

if [ "$hits" -ne 0 ]; then
  echo "check-md-semicolon: split the sentence or reword, one sentence per line" >&2
  exit 1
fi
echo "check-md-semicolon: clean"
