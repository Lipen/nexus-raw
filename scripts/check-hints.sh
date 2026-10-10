#!/bin/sh
# Cross-check the hint prose: every hint literal in `Error::hint` (error.rs)
# must appear in docs/reference/errors.md, and vice versa: a drift between
# the source and the docs means the docs lie about what the binary prints.
#
# Called from `just check-docs` and the docs workflow.
set -eu

src=crates/nexus-raw-core/src/error.rs
doc=docs/reference/errors.md
failed=0

# Extract the hint literals from the source: the string arguments of the
# hint match arms, joined continuation lines included. The scan starts at
# `pub fn hint` so test fixtures and other Some() strings stay out.
src_hints="$(awk '/pub fn hint/,/^    }$/' "$src" | perl -0ne 'while (/Some\(\s*\n?\s*"((?:[^"\\]|\\.)*)"/g) { print "$1\n" }')"

# Every source hint must appear in the docs verbatim (a hint may be split
# across md lines, so compare against the page text with newlines collapsed).
doc_flat="$(tr '\n' ' ' < "$doc")"
while IFS= read -r hint; do
  [ -n "$hint" ] || continue
  hint_flat="$(printf '%s' "$hint" | tr '\n' ' ')"
  case "$doc_flat" in
    *"$hint_flat"*) ;;
    *)
      echo "check-hints: the docs miss the hint: $hint" >&2
      failed=1
      ;;
  esac
done <<EOF
$src_hints
EOF

[ "$failed" = 0 ] || { echo "check-hints: see crates/nexus-raw-core/src/error.rs vs $doc" >&2; exit 1; }
echo "check-hints: every error hint is documented"
