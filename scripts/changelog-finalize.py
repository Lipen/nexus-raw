#!/usr/bin/env python3
"""Finalize the `[Unreleased]` changelog section into `[X.Y.Z]`.

Idempotent.
A section that already exists only moves to today's date, so a re-release
after a failed attempt re-dates the release instead of duplicating it.
The compare links follow the section.
Called from `just release`.
"""

import datetime
import re
import sys


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: changelog-finalize.py X.Y.Z", file=sys.stderr)
        return 2
    v = sys.argv[1]
    path = "CHANGELOG.md"
    s = open(path).read()
    today = datetime.date.today().isoformat()
    if f"## [{v}]" in s:
        pat = r"## \[" + re.escape(v) + r"\] - \d{4}-\d{2}-\d{2}"
        s = re.sub(pat, f"## [{v}] - {today}", s, count=1)
        print(f"changelog: [{v}] moves to {today}")
    else:
        if "## [Unreleased]" not in s:
            print("changelog-finalize: no [Unreleased] section to finalize", file=sys.stderr)
            return 1
        s = s.replace("## [Unreleased]", f"## [Unreleased]\n\nNothing yet.\n\n## [{v}] - {today}", 1)
        m = re.search(r"\[Unreleased\]: (\S+/compare/)(v[\d.]+)\.\.\.HEAD", s)
        if not m:
            print("changelog-finalize: the [Unreleased] compare link is missing", file=sys.stderr)
            return 1
        s = s.replace(
            m.group(0),
            f"[Unreleased]: {m.group(1)}v{v}...HEAD\n[{v}]: {m.group(1)}{m.group(2)}...v{v}",
            1,
        )
        print(f"changelog: [Unreleased] finalized as [{v}] - {today}")
    open(path, "w").write(s)
    return 0


if __name__ == "__main__":
    sys.exit(main())
