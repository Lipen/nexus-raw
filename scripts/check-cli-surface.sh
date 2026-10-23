#!/bin/sh
# Cross-check the CLI flag surface against the pages that teach it: every
# `--flag` a page names must exist in the clap definition, and every flag the
# clap definition carries must be taught by a page or sit in the exemption
# table below with a reason. The skill ships to agents and a page teaching a
# flag the binary rejects is a broken page, so a hand fix in one file must not
# be able to rot in the other.
#
# The clap attributes are the source of truth, parsed out of the binary's
# source the way check-hints.sh parses the hint literals out of error.rs.
#
# Called from `just check-docs`: the scan needs the whole tree at once (the
# flag source and the pages), not the staged subset a prek hook receives.
set -eu

usage() {
  echo "usage: check-cli-surface.sh"
  echo "  cross-checks the clap flags against docs/reference/cli.md, docs/how-to/*.md"
  echo "  and skills/nxr; exits 1 on drift"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

src=crates/nexus-raw/src/main.rs
for required in "$src" docs/reference/cli.md; do
  [ -f "$required" ] || { echo "check-cli-surface: no $required (run from the repo root)" >&2; exit 1; }
done

failed=0
fail() {
  echo "check-cli-surface: $1" >&2
  failed=1
}

# Whole-token membership over a newline- or space-delimited list: the lists
# are flattened, then both ends are wrapped so `q` cannot match inside
# `quiet`.
contains() {
  case " $(printf '%s' "$1" | tr '\n' ' ') " in
    *" $2 "*) ;;
    *) return 1 ;;
  esac
}

# The surface: every `#[arg(...)]` that names a long flag, plus the aliases it
# accepts as spellings. The long name is explicit (`long = "no-sha"`) or the
# kebab-cased field name (bare `long`). `#[arg(skip)]` fields never reach
# argv. The cmd modules are scanned too: a flag declared there belongs to the
# same surface.
surface="$(perl -0777 -ne '
  while (/\[arg\(((?:[^()]|\([^()]*\))*)\)\]\s*(?:pub\(crate\)\s*)?([a-z_][a-z0-9_]*)\s*:/g) {
    my ($attrs, $field) = ($1, $2);
    next if $attrs =~ /\bskip\b/;
    if ($attrs =~ /(?:^|[,(])\s*long\s*=\s*"([^"]*)"/m) { print "flag $1\n" }
    elsif ($attrs =~ /(?:^|[,(])\s*long(?:\s*[,)]|\s*$)/m) { (my $l = $field) =~ s/_/-/g; print "flag $l\n" }
    while ($attrs =~ /(?:visible_)?alias\s*=\s*"([^"]*)"/g) { print "alias $1\n" }
  }
' "$src" crates/nexus-raw/src/cmd/*.rs | sort -u)"

flags="$(printf '%s\n' "$surface" | sed -n 's/^flag //p')"
aliases="$(printf '%s\n' "$surface" | sed -n 's/^alias //p')"
[ -n "$flags" ] || { echo "check-cli-surface: no flags parsed out of $src" >&2; exit 1; }

# clap adds these itself; a page naming one of them is not lying.
builtins="help
version"

# Long flags the pages name that belong to another tool: a token here is not a
# claim about nxr, and the owner on the row is the reason. A page may teach
# another tool's flag; it may never teach nxr a flag it does not have.
foreign="cargo --path --workspace
npm --registry
curl --proxy
docker --from --network --password-stdin --platform --rm --user
scripts/install-skill.sh --agent --dir --link --list --project
scripts/brew-formula.sh --check
mock-nexus --chunk-delay-ms --chunk-size --flaky --port --print-scenarios
the wasm sandbox --allow-host --max-request-bytes --max-response-bytes --rate-limit --upstream"
foreign_flags="$(printf '%s\n' "$foreign" | grep -oE -- '--[a-z0-9-]+' | sed 's/^--//' | sort -u)"

# Flags the gate lets a page omit. One row per flag: `--flag reason`, the
# reason on the same line, because an exemption without one is a bug in this
# script. Empty today: every flag the binary carries is taught. An alias is
# not a row here: its canonical spelling covers it.
exempt=""
exempted=""
while IFS= read -r row; do
  [ -n "$row" ] || continue
  ex_flag="${row%% *}"
  case "$ex_flag" in
    --?*) ;;
    *) fail "an exemption row must open with --flag: $row"; continue ;;
  esac
  case "$row" in
    *" "*) ex_reason="${row#* }" ;;
    *) ex_reason="" ;;
  esac
  [ -n "$ex_reason" ] || fail "the exemption for $ex_flag carries no reason: $row"
  ex_name="${ex_flag#--}"
  contains "$flags" "$ex_name" || fail "the exemption for $ex_flag names no flag the CLI carries"
  exempted="$exempted $ex_name"
done <<EOF
$exempt
EOF

# The pages that teach the CLI: the reference, the how-tos and the shipped
# skill. A page outside this set is outside the gate's contract.
known="$(printf '%s\n%s\n%s\n%s\n' "$flags" "$aliases" "$builtins" "$foreign_flags")"
documented=""
for page in docs/reference/cli.md docs/how-to/*.md skills/nxr/SKILL.md skills/nxr/references/*.md; do
  [ -f "$page" ] || continue
  tokens="$(grep -oE -- '--[a-z][a-z0-9-]*' "$page" | sort -u)"
  while IFS= read -r token; do
    name="${token#--}"
    [ -n "$name" ] || continue
    documented="$documented $name"
    contains "$known" "$name" || fail "$page teaches $token, which the CLI does not accept"
  done <<EOF
$tokens
EOF
done

# The skill tree may grow a subdirectory: a page the glob misses is a page the
# gate never reads, so name it here instead of skipping it silently.
stray="$(find skills/nxr -name '*.md' ! -path 'skills/nxr/SKILL.md' ! -path 'skills/nxr/references/*' | sort)"
[ -z "$stray" ] || fail "skill pages outside the gate's glob, extend the page scan: $stray"

# The reverse pass: an undocumented flag is invisible to every reader.
for flag in $flags; do
  contains "$exempted" "$flag" && continue
  contains "$documented" "$flag" && continue
  fail "--$flag is taught nowhere: document it in docs/reference/cli.md or exempt it here"
done

[ "$failed" = 0 ] || { echo "check-cli-surface: see $src vs the CLI pages" >&2; exit 1; }
echo "check-cli-surface: $(printf '%s\n' "$flags" | wc -l | tr -d ' ') flags agree with the pages"
