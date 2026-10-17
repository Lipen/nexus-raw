# Teach an agent the skill

`nxr` ships an Agent Skill: a `SKILL.md` that teaches a coding agent how to drive the tool.
It carries the command map, the machine contract, the recipes and the server quirks that are easy to get wrong.
The same files work for every agent that understands the format, and they live in the repository under [`skills/nxr/`](https://github.com/Lipen/nexus-raw/tree/master/skills/nxr).

## What is in it

| File | What it carries |
|:--|:--|
| `SKILL.md` | the trigger, the command map, the machine contract, the top server facts |
| `references/recipes.md` | step-by-step tasks: publish, resume, mirror, promote a channel, delete, audit, gate a pipeline |
| `references/quirks.md` | the full list of server behaviours that look like bugs and are not |

An agent loads `SKILL.md` first and reaches into a reference only when the task needs it, which keeps the always-loaded part small.

## Install it from a checkout

The installer detects your agent from the home directory and copies the skill where that agent reads skills.

```console
$ scripts/install-skill.sh
install-skill: copied the nxr skill
  from  …/skills/nxr
  to    ~/.claude/skills/nxr
  agent claude
start a fresh session for the agent to pick it up
```

Pick the target explicitly when auto-detection guesses wrong.

```console
$ scripts/install-skill.sh --list
known agents (user scope / project scope):
  claude   ~/.claude/skills  …/.claude/skills
  omp      ~/.omp/agent/skills  …/.omp/skills
$ scripts/install-skill.sh --agent omp
$ scripts/install-skill.sh --agent claude --project
```

`--project` installs into the current project instead of the home directory.
`--link` symlinks to the checkout rather than copying, so the installed skill tracks the repository.
When an agent is not in the table, name any directory with `--dir`.

```console
$ scripts/install-skill.sh --dir ~/.config/my-agent/skills
```

## Install it without a checkout

The skill is one file plus a folder, so copying works when a checkout is not at hand.

```console
$ base=https://raw.githubusercontent.com/Lipen/nexus-raw/master/skills/nxr
$ dest=~/.claude/skills/nxr
$ mkdir -p "$dest/references"
$ curl -fsSL "$base/SKILL.md" -o "$dest/SKILL.md"
$ curl -fsSL "$base/references/recipes.md" -o "$dest/references/recipes.md"
$ curl -fsSL "$base/references/quirks.md" -o "$dest/references/quirks.md"
```

The release archives carry the same `skills/` directory next to the binary, so a binary download gives you both.

## Keep it fresh

The skill is versioned with the tool, so a stale skill can describe flags that moved.
Reinstall after an upgrade, or install with `--link` and pull.
The repository keeps the file honest with a gate, `scripts/check-skill.sh`, that holds the frontmatter loadable and every relative link resolving.
