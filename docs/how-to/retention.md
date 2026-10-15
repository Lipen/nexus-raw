# Keep the newest versions

Retention is a policy, and policies live on your side.
`nxr` deliberately ships no retention command: it cannot guess which directory segments are versions, and a destructive command must never guess its scope.
The recipe below builds keep-N retention from ordinary commands, with explicit enumeration every step.

The shape it assumes is yours: version directories under one base, and channel refs at that base naming versions that must survive.
If your tree looks different, change the recipe: it is three commands and a loop.

## The recipe

```bash
BASE=https://nexus.example.com/repository/raw-main/releases/
KEEP=3

# The channel guard: never delete a version a ref still names.
PINNED=$(nxr channel get "$BASE/latest" 2>/dev/null || true)

# The versions, newest last, are plain directory names from `ls`.
versions=($(nxr ls "$BASE" | sed -e '/\/$/!d' -e 's:/*$::'))
count=${#versions[@]}

# Sort them however your scheme orders them; dotted-numeric example:
sorted=($(printf '%s\n' "${versions[@]}" | sort -V))

for ((i = 0; i < count - KEEP; i++)); do
  v=${sorted[$i]}
  if [ "$v" = "$PINNED" ]; then
    echo "skip $v (channel-pinned)"
    continue
  fi
  nxr rm "$BASE$v/" --ls --yes
done
```

## Why these pieces

- `nxr ls` prints the tree you actually have, folders marked with `/`.
  Nothing interprets a segment as a version: you see names, you pick names.
- `sort -V` is your ordering decision, spelled in your script.
  Swap it for `sort`, `sort -r` or a timestamp column: the tool does not care.
- `nxr rm --ls --yes` deletes exactly the enumerated names of that one directory.
  Without `--yes` it prints the plan and deletes nothing, so drop the flag while shaping the loop.

## The channel guard

`channel get` reads one token file.
The guard is three lines of shell because that is all it takes:

```bash
PINNED=$(nxr channel get "$BASE/latest" 2>/dev/null || true)
[ "$v" = "$PINNED" ] && { echo "skip $v"; continue; }
```

One pinned token is the common case.
Several refs (`stable`, `rc`, `nightly`) mean several `channel get` calls collected into a list, and the skip test becomes a lookup.
