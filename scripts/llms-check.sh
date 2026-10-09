#!/bin/sh
# Assert that docs/llms.txt keeps the llmstxt.org structure: an H1 first, then
# prose, then only H2 headings whose sections are `- [name](url): note` link
# lists, with no heading level in between and no URL named twice.
#
# Called from `just check-docs` and from the docs workflow: the file is the
# machine entry point for agents, and a structural drift there is invisible
# to humans.
set -eu

f=docs/llms.txt
failed=0

head -n 1 "$f" | grep -q '^# ' || { echo "llms-check: the first line is not an H1" >&2; failed=1; }

# Only line 1 may be an H1 and nothing may dive past H2: the spec's parser
# treats H2 as the section delimiter and deeper headings as noise.
if strays="$(grep -nE '^# |^#{3,}' "$f" | grep -v '^1:')"; then
  echo "llms-check: a heading is not H2 (or a second H1):" >&2
  echo "$strays" >&2
  failed=1
fi

# Every bullet inside an H2 section must be a markdown link with an https URL:
# link extraction is the whole point of the format.
if bad="$(sed -n '/^## /,$p' "$f" | grep -E '^- ' | grep -vE '^- \[[^]]+\]\(https://[^)]+\)' )"; then
  echo "llms-check: an H2 bullet is not a [name](https://...) link:" >&2
  echo "$bad" >&2
  failed=1
fi

if dup="$(grep -oE 'https://[^)]+' "$f" | sort | uniq -d)" && [ -n "$dup" ]; then
  echo "llms-check: a URL appears twice:" >&2
  echo "$dup" >&2
  failed=1
fi

[ "$failed" = 0 ] || exit 1
echo "llms-check: $(grep -c '^- \[' "$f") links conform"
