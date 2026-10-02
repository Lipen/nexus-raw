#!/usr/bin/env python3
"""Prepare a recording for the two things that replay it: the landing player
and the landing SVG.

`demo.sh` writes a recording: every line with the time it arrived. A local
transfer prints a dozen lines in under a second, so replayed as recorded it is
a blur, and both renderers would have to invent their own pace and their own
line colors — two chances to drift apart. This writes the display data into the
cast instead:

  title   the window title both renderers show
  at      when each line appears on screen, ms from the start
  kinds   what each line is (cmd, ok, err, ...), the CSS/SVG class suffixes

The model behind `at` is small on purpose:

  hold(line) = BASE + PER_CHAR * length          clamped to [MIN, MAX]
  hold("")   = BLANK                             a blank line is a beat
  + CMD for `$ ...`                              a command reads, then output starts
  + ERROR for `error:` and `hint:`               the lines that have to be parsed

The recorded timestamps stay in `lines` untouched: what happened is history,
when it is shown is a display decision. A real pause longer than a reader needs
is not reproduced — the hero has no dead air.

Run by `just demo-cast`.
"""

import json
import re
import sys

START_MS = 500  # a beat before the first line lands
BASE_MS = 200
PER_CHAR_MS = 5
MIN_MS = 380
MAX_MS = 1400
BLANK_MS = 550
CMD_MS = 300
ERROR_MS = 200
# after the last line, the finished transcript stays up, then the screen
# clears and the run starts over: the loop the readme animation plays
TAIL_MS = 2500

# The class suffixes the stylesheet and the SVG palette both key on.
KINDS = (
    (re.compile(r"^#"), "comment"),
    (re.compile(r"^\$ "), "cmd"),
    (re.compile(r"^error:"), "err"),
    (re.compile(r"^hint:"), "hint"),
    (re.compile(r"^↻"), "warn"),
    (re.compile(r"^(→|○)"), "dim"),
    (re.compile(r"^[↑↓]"), "ok"),
    (re.compile(r"^(plan|channel|verify|uploaded|failed):"), "note"),
)


def kind(text: str) -> str:
    for pattern, name in KINDS:
        if pattern.match(text):
            return name
    return ""


def hold(text: str) -> int:
    if not text:
        return BLANK_MS
    ms = BASE_MS + PER_CHAR_MS * len(text)
    if text.startswith("$ "):
        ms += CMD_MS
    if text.startswith(("error:", "hint:")):
        ms += ERROR_MS
    return max(MIN_MS, min(MAX_MS, ms))


def encode(cast: dict) -> str:
    """The cast file, one line per recorded line: the diff stays readable."""
    head = ",".join(
        f'"{key}":{json.dumps(cast[key], ensure_ascii=False)}'
        for key in ("v", "scenario", "version", "tool", "title", "cycle")
    )
    lines = ",\n".join(json.dumps(line, ensure_ascii=False) for line in cast["lines"])
    at = ", ".join(str(t) for t in cast["at"])
    kinds = ", ".join(json.dumps(k, ensure_ascii=False) for k in cast["kinds"])
    return f'{{{head},"lines":[\n{lines}\n],"at":[\n{at}\n],"kinds":[\n{kinds}\n]}}\n'


def main(path: str) -> int:
    with open(path, encoding="utf-8") as handle:
        cast = json.load(handle)

    lines = cast["lines"]
    cast["title"] = " · ".join(
        part for part in (cast["tool"], cast["scenario"] and f"against mock-nexus ({cast['scenario']})") if part
    )

    at, kinds, clock = [], [], START_MS
    for line in lines:
        at.append(clock)
        kinds.append(kind(line[1]))
        clock += hold(line[1])
    cast["at"], cast["kinds"] = at, kinds
    cast["cycle"] = clock + TAIL_MS

    with open(path, "w", encoding="utf-8") as handle:
        handle.write(encode(cast))

    blanks = sum(1 for line in lines if not line[1])
    print(
        f"stage: {len(lines)} lines, {blanks} beats, {clock / 1000:.1f}s on screen,"
        f" {cast['cycle'] / 1000:.1f}s per loop (avg {clock // len(lines)}ms a line)"
    )
    return 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(2)
    sys.exit(main(sys.argv[1]))
