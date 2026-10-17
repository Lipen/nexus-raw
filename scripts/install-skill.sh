#!/bin/sh
# Install the nxr agent skill into an agent's skills directory.
#
# The skill itself lives in skills/nxr/ and is the single source of truth: this
# script only places it where an agent looks for skills. Copy is the default,
# so the installed skill keeps working after the repository moves.
set -eu

usage() {
  echo "usage: install-skill.sh [--list | --dir DIR | --agent NAME] [--project] [--link]"
  echo "  places skills/nxr/ (SKILL.md and references/) where an agent reads skills"
  echo ""
  echo "options:"
  echo "  --dir DIR     install into DIR/nxr"
  echo "  --agent NAME  pick a known agent (see --list)"
  echo "  --project     install into the current project instead of the home directory"
  echo "  --link        symlink to this checkout instead of copying, so it tracks the repo"
  echo "  --list        print the known agents and exit"
  echo ""
  echo "with no flags the agent is auto-detected from the home directory"
}
usage_err() {
  usage >&2
  exit 2
}

# name<TAB>home marker<TAB>user-scope dir<TAB>project-scope dir.
# Every path is relative, so the table stays free of the environment and the
# use sites do the prefixing.
agents() {
  cat <<'EOF'
claude	.claude	.claude/skills	.claude/skills
omp	.omp	.omp/agent/skills	.omp/skills
EOF
}

agent_field() {
  agents | awk -F'\t' -v want="$1" -v col="$2" '$1 == want { print $col }'
}

agent_names() {
  agents | awk -F'\t' '{ print $1 }'
}

known_agent() {
  [ -n "$(agent_field "$1" 3)" ]
}

list_agents() {
  echo "known agents (user scope / project scope):"
  agents | awk -F'\t' -v home="$HOME" -v pwd="$PWD" '{ printf "  %-8s %s/%s  %s/%s\n", $1, home, $3, pwd, $4 }'
}

root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/skills/nxr"

dir=""
agent=""
link=0
project=0

while [ $# -gt 0 ]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    --list) list_agents; exit 0 ;;
    --dir) [ $# -ge 2 ] || usage_err; dir="$2"; shift 2 ;;
    --agent) [ $# -ge 2 ] || usage_err; agent="$2"; shift 2 ;;
    --project) project=1; shift ;;
    --link) link=1; shift ;;
    --copy) link=0; shift ;;
    *) usage_err ;;
  esac
done

if [ ! -f "$src/SKILL.md" ]; then
  echo "install-skill: $src/SKILL.md is missing, run this from the repository" >&2
  exit 1
fi

if [ -n "$agent" ]; then
  if ! known_agent "$agent"; then
    echo "install-skill: unknown agent '$agent'" >&2
    list_agents >&2
    exit 2
  fi
  if [ "$project" -eq 1 ]; then
    dir="$PWD/$(agent_field "$agent" 4)"
  else
    dir="$HOME/$(agent_field "$agent" 3)"
  fi
elif [ -z "$dir" ]; then
  # Auto-detect: the first agent whose home marker exists wins.
  for candidate in $(agent_names); do
    if [ -d "$HOME/$(agent_field "$candidate" 2)" ]; then
      dir="$HOME/$(agent_field "$candidate" 3)"
      agent="$candidate"
      break
    fi
  done
fi

if [ -z "$dir" ]; then
  echo "install-skill: no agent directory found under \$HOME" >&2
  echo "pass --dir DIR, or --agent NAME to pick one explicitly" >&2
  list_agents >&2
  exit 1
fi

dest="$dir/nxr"

mkdir -p "$dir"
rm -rf "$dest"
if [ "$link" -eq 1 ]; then
  ln -s "$src" "$dest"
  how="linked"
else
  mkdir -p "$dest"
  cp -R "$src/." "$dest/"
  how="copied"
fi

echo "install-skill: $how the nxr skill"
echo "  from  $src"
echo "  to    $dest"
if [ -n "$agent" ]; then
  echo "  agent $agent"
fi
echo "start a fresh session for the agent to pick it up"
