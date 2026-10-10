#!/bin/sh
# Markdown prose must not splice sentences with `;`: the repo style is one
# sentence per line, sentences end with a period. Code blocks and inline code
# are exempt: a semicolon there is syntax, not typography.
set -eu

hits=0
# Tracked markdown only: git ls-files keeps node_modules, target and site out
# by construction, and untracked scratch files never gate.
while IFS= read -r -d '' file; do
  # Strip fenced code blocks first, then look for the splice pattern:
  # a lowercase/uppercase word, `; `, and another word on the same line.
  matches=$(awk '
    /^```/ { fence = !fence; next }
    !fence && /; / && !/`[^`]*;[^`]*`/ { print FILENAME ":" FNR ": " $0 }
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
