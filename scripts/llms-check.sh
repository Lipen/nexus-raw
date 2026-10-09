#!/bin/sh
# Assert that docs/llms.txt keeps the llmstxt.org structure: an H1 first, then
# prose, then only H2 headings whose sections are `- [name](url): note` link
# lists, with no heading level in between and no URL named twice.
#
# Called from `just check-docs` and from the docs workflow: the file is the
# machine entry point for agents, and a structural drift there is invisible
# to humans.
set -eu

f="${1:-docs/llms.txt}"
failed=0

head -n 1 "$f" | grep -q '^# ' || { echo "llms-check: the first line is not an H1" >&2; failed=1; }

# At least one H2 section must exist: an empty or truncated file would
# otherwise pass vacuously, and the drift it hides is the point of the gate.
grep -q '^## ' "$f" || { echo "llms-check: no H2 section at all" >&2; failed=1; }

# Only line 1 may be an H1 and nothing may dive past H2: the spec's parser
# treats H2 as the section delimiter and deeper headings as noise.
# Fenced blocks are skipped: a shell comment inside an example is not a heading.
if strays="$(awk '/^```/{f=!f; next} !f && NR>1 && ($0 ~ /^# / || $0 ~ /^#{3,}/) {print NR":"$0}' "$f")" && [ -n "$strays" ]; then
  echo "llms-check: a heading is not H2 (or a second H1):" >&2
  echo "$strays" >&2
  failed=1
fi

# Every bullet inside an H2 section must be a markdown link with an https URL:
# link extraction is the whole point of the format.
# Nested (indented) bullets count too: a renderer still shows them.
if bad="$(sed -n '/^## /,$p' "$f" | grep -E '^[[:space:]]*-' | grep -vE '^[[:space:]]*- \[[^]]+\]\(https://[^)]+\)' )"; then
  echo "llms-check: an H2 bullet is not a [name](https://...) link:" >&2
  echo "$bad" >&2
  failed=1
fi

# A URL may repeat across the file only in prose: the link lists are the
# machine surface, and a repeated entry there means two names for one page.
# The extraction anchors on the link itself, so a URL quoted in a note or
# wrapped in parens does not collide with its own entry.
# The extraction balances parens: a URL may carry (grouped) segments, and the
# link ends at the paren that closes the markdown syntax, not the first one.
if dup="$(sed -n '/^## /,$p' "$f" | grep -E '^[[:space:]]*- \[' | awk '{
    if (match($0, /\]\(/)) {
        rest = substr($0, RSTART + RLENGTH)
        depth = 0
        url = ""
        for (i = 1; i <= length(rest); i++) {
            c = substr(rest, i, 1)
            if (c == "(") depth++
            else if (c == ")") { if (depth == 0) break; depth-- }
            url = url c
        }
        print url
    }
}' | grep '^https:' | sort | uniq -d)" && [ -n "$dup" ]; then
  echo "llms-check: a link entry repeats a URL:" >&2
  echo "$dup" >&2
  failed=1
fi

[ "$failed" = 0 ] || exit 1
echo "llms-check: $(sed -n '/^## /,$p' "$f" | grep -cE '^[[:space:]]*- \[') links conform"
