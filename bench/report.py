#!/usr/bin/env python3
"""Turn a criterion run into a durable record, and compare two records.

The benches write their measurements under `bench/target/criterion/<group>/<case>/<baseline>/estimates.json`,
where `<baseline>` is the tag a run was saved under and `target/` dies with `cargo clean`.
That makes criterion's own baseline a working surface, not a record.
This script is the record: `bench/reports/<tag>.json` holds the numbers, `bench/reports/<tag>.md`
renders them, and `compare` reads two records back into a delta table.

    just bench-report first        # run the suite, save it as `first`, write both files
    just bench-report second       # the same after a change
    just bench-compare first second

Manual and opt-in, like the benches: nothing here runs in CI.
"""

from __future__ import annotations

import json
import os
import platform
import re
import subprocess
import sys
from datetime import UTC, datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
# `just bench-report` exports CRITERION_HOME at the repo-local tree, so the run
# and the collector cannot disagree. The default is the same path.
CRITERION = Path(os.environ.get("CRITERION_HOME") or HERE / "target" / "criterion")
REPORTS = HERE / "reports"

# The dataset tag `TreeSpec::tag` writes: the report reads the shape back out of it.
DATASET = re.compile(
    r"^(?P<files>\d+)f-(?P<depth>\d+)d-(?P<fanout>\d+)w-(?P<size>\d+)b-seed(?P<seed>\d+)$"
)
# The knobs a case name carries, plus the worker count that follows the tag.
# A record reads them back out of the cases, never out of the shell that
# happened to run `collect`: two records are comparable only when these agree.
KNOBS = ("files", "depth", "fanout", "size", "seed")
DIMENSIONS = (*KNOBS, "workers")
# A delta below this reads as noise on a shared machine.
NOISE_PERCENT = 5.0
# A tag names a baseline directory. Criterion keeps its own directories beside
# the baselines (`new` is a symlink to the newest run, `change` and `report` are
# its own reports), so those names would collect a harness artifact as a record.
TAG = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
RESERVED = ("new", "change", "report")


def fail(message: str) -> None:
    """Stop with a message on stderr."""
    print(f"report: {message}", file=sys.stderr)
    raise SystemExit(2)


def capture(*argv: str) -> str:
    """Run a command and return its stdout, or an empty string when it is not there."""
    try:
        out = subprocess.run(argv, capture_output=True, text=True, check=True, cwd=HERE.parent)
    except (OSError, subprocess.CalledProcessError):
        return ""
    return out.stdout.strip()


def criterion_version() -> str:
    """The resolved criterion, read from the bench lock so a report names the harness it used."""
    lock = HERE / "Cargo.lock"
    if not lock.is_file():
        return "unknown"
    lines = lock.read_text().splitlines()
    for index, line in enumerate(lines):
        if line == 'name = "criterion"':
            for follow in lines[index + 1 : index + 3]:
                if follow.startswith("version = "):
                    return follow.split('"')[1]
    return "unknown"


def cpu_model() -> str:
    """The CPU the numbers belong to: a shared machine makes them worthless without it."""
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text().splitlines():
            if line.startswith("model name"):
                return line.split(":", 1)[1].strip()
    return platform.processor() or "unknown"


def metadata() -> dict[str, object]:
    """What a number needs to be comparable: the commit, the toolchain, the machine."""
    return {
        "recorded": datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "commit": capture("git", "rev-parse", "--short", "HEAD") or "unknown",
        "dirty": bool(capture("git", "status", "--porcelain")),
        "rustc": capture("rustc", "--version") or "unknown",
        "criterion": criterion_version(),
        "platform": f"{platform.system().lower()} {platform.machine()}",
        "cpu": cpu_model(),
    }


def shape(name: str) -> tuple[int, int] | None:
    """The files and bytes-per-file a case name carries, or None when it carries neither."""
    for part in name.split(" / "):
        match = DATASET.match(part)
        if match:
            return int(match["files"]), int(match["size"])
    return None


def workers(name: str) -> int | None:
    """The worker count of a case name: the component after the dataset tag, when it is a number."""
    parts = name.split(" / ")
    for index, part in enumerate(parts):
        if DATASET.match(part) and index + 1 < len(parts) and parts[index + 1].isdigit():
            return int(parts[index + 1])
    return None


def parameters(cases: list[dict[str, object]]) -> dict[str, str]:
    """The ladders the recorded cases cover, read out of the case names themselves.

    The shell a run was launched from is not part of the record: `collect` may be
    re-run from anywhere, and a blank reading there would let the comparability
    guard pass two different datasets as equal.
    """
    seen: dict[str, set[int]] = {knob: set() for knob in DIMENSIONS}
    for case in cases:
        for part in str(case["name"]).split(" / "):
            match = DATASET.match(part)
            if match:
                for knob in KNOBS:
                    seen[knob].add(int(match[knob]))
        worker = case.get("workers")
        if isinstance(worker, int):
            seen["workers"].add(worker)
    if not any(seen[knob] for knob in DIMENSIONS):
        return {"ladder": "unknown"}
    return {knob: ",".join(str(v) for v in sorted(values)) for knob, values in seen.items()}


def nanoseconds(estimate: object) -> float:
    """The point estimate of one criterion statistic, in nanoseconds.

    A statistic criterion could not compute comes back as null (a slope with too
    few samples, for one), and a report reads that as zero rather than crashing.
    """
    if not isinstance(estimate, dict):
        return 0.0
    point = estimate.get("point_estimate")
    return float(point) if isinstance(point, (int, float)) else 0.0


def collect(tag: str) -> int:
    """Read the measurements saved under `tag` into reports/<tag>.json and .md."""
    if not TAG.match(tag) or tag in RESERVED:
        fail(
            f"'{tag}' is not a usable tag: letters, digits, dashes and dots, "
            f"not one of {', '.join(RESERVED)}"
        )
    cases = []
    if CRITERION.is_dir():
        for path in sorted(CRITERION.rglob("estimates.json")):
            if path.parent.name != tag:
                continue
            estimates = json.loads(path.read_text())
            name = " / ".join(path.relative_to(CRITERION).parts[:-2])
            dimensions = shape(name)
            files, size = dimensions if dimensions else (None, None)
            mean = nanoseconds(estimates.get("mean", {}))
            case = {
                "name": name,
                "group": name.split(" / ")[0],
                "files": files,
                "size": size,
                "workers": workers(name),
                "mean_ns": mean,
                "median_ns": nanoseconds(estimates.get("median", {})),
                "std_dev_ns": nanoseconds(estimates.get("std_dev", {})),
                "slope_ns": nanoseconds(estimates.get("slope", {})),
            }
            if files and size and mean > 0:
                per_second = files / (mean / 1e9)
                case["objects_per_second"] = per_second
                case["mib_per_second"] = (files * size) / (mean / 1e9) / (1024 * 1024)
            cases.append(case)
    if not cases:
        fail(f"no measurements saved under `{tag}`: run `just bench-report {tag}`")
    record = {"tag": tag, "cases": cases, "parameters": parameters(cases), **metadata()}
    REPORTS.mkdir(exist_ok=True)
    (REPORTS / f"{tag}.json").write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    (REPORTS / f"{tag}.md").write_text(render(record))
    print(f"report: {len(cases)} cases written to reports/{tag}.json and reports/{tag}.md")
    return 0


def duration(value_ns: float) -> str:
    """A duration a human reads at a glance."""
    if value_ns >= 1e9:
        return f"{value_ns / 1e9:.2f} s"
    if value_ns >= 1e6:
        return f"{value_ns / 1e6:.1f} ms"
    if value_ns >= 1e3:
        return f"{value_ns / 1e3:.1f} µs"
    return f"{value_ns:.0f} ns"


def render(record: dict[str, object]) -> str:
    """The markdown of one record: the machine, the numbers, and the arithmetic they support."""
    cases = record["cases"]
    assert isinstance(cases, list)
    lines = [f"# Bench report: {record['tag']}", ""]
    lines += ["## Machine", "", "| Field | Value |", "|:--|:--|"]
    for key in ("recorded", "commit", "dirty", "rustc", "criterion", "platform", "cpu"):
        lines.append(f"| {key} | {record[key]} |")
    parameters = record["parameters"]
    assert isinstance(parameters, dict)
    knobs = ", ".join(f"`{key}={value}`" for key, value in sorted(parameters.items()))
    lines.append(f"| parameters | {knobs or 'defaults'} |")
    lines += ["", f"Every number is the mean over one criterion run; the units are per operation.", ""]
    lines += ["## Timings", "", "| Benchmark | Mean | Median | Std dev | Objects/s | MiB/s |", "|:--|--:|--:|--:|--:|--:|"]
    for case in cases:
        per_second = case.get("objects_per_second")
        mib = case.get("mib_per_second")
        lines.append(
            "| {name} | {mean} | {median} | {std} | {ops} | {mib} |".format(
                name=case["name"],
                mean=duration(case["mean_ns"]),
                median=duration(case["median_ns"]),
                std=duration(case["std_dev_ns"]),
                ops=f"{per_second:,.0f}" if per_second else "",
                mib=f"{mib:.1f}" if mib else "",
            )
        )
    lines += ["", "## Reading", "", "Arithmetic over the table above, nothing more.", ""]
    lines += reading(cases)
    return "\n".join(lines) + "\n"


def reading(cases: list[dict[str, object]]) -> list[str]:
    """The few facts the numbers state outright: the leaders, and how the ladders scale."""
    lines = []
    groups: dict[str, list[dict[str, object]]] = {}
    for case in cases:
        group = str(case["group"])
        groups.setdefault(group, []).append(case)
    for group, members in sorted(groups.items()):
        with_bytes = [case for case in members if case.get("mib_per_second")]
        if with_bytes:
            best = max(with_bytes, key=lambda case: case["mib_per_second"])
            lines.append(
                f"- `{group}`: the fastest case is `{best['name']}` at {best['mib_per_second']:.1f} MiB/s."
            )
        else:
            best = min(members, key=lambda case: case["mean_ns"])
            lines.append(
                f"- `{group}`: the fastest case is `{best['name']}` at {duration(best['mean_ns'])}."
            )
        ladder = ladder_scaling(members)
        if ladder:
            lines.append(f"- `{group}`: {ladder}.")
    return lines


def ladder_scaling(members: list[dict[str, object]]) -> str:
    """How the time moves along the worker ladder, at a fixed dataset."""
    ladders: dict[tuple[object, object], list[dict[str, object]]] = {}
    for case in members:
        if case["workers"] is not None:
            ladders.setdefault((case["files"], case["size"]), []).append(case)
    notes = []
    for (files, _size), rungs in sorted(ladders.items(), key=lambda item: str(item[0])):
        rungs = sorted(rungs, key=lambda case: case["workers"])
        if len(rungs) < 2:
            continue
        slowest, fastest = rungs[0], rungs[-1]
        if slowest["mean_ns"] <= 0:
            continue
        factor = slowest["mean_ns"] / fastest["mean_ns"]
        notes.append(
            f"{files} files: {slowest['workers']} workers {duration(slowest['mean_ns'])} "
            f"→ {fastest['workers']} workers {duration(fastest['mean_ns'])} ({factor:.2f}×)"
        )
    return "; ".join(notes)


def load(tag: str) -> dict[str, object]:
    """One stored record, by tag."""
    path = REPORTS / f"{tag}.json"
    if not path.is_file():
        fail(f"no record at {path}: run `just bench-report {tag}` first")
    record = json.loads(path.read_text())
    if not isinstance(record, dict) or "cases" not in record:
        fail(f"{path} is not a report record")
    return record


def compare(first: str, second: str) -> int:
    """Write reports/<first>-vs-<second>.md and print the delta table."""
    left, right = load(first), load(second)
    cases = {str(case["name"]): case for case in left["cases"]}
    for case in right["cases"]:
        cases.setdefault(str(case["name"]), None)
    lines = [f"# Bench delta: {first} → {second}", ""]
    warnings = comparability(left, right)
    for warning in warnings:
        lines += [f"> **{warning}**", ""]
    lines += [
        f"- `{first}`: {left['recorded']} at {left['commit']}, {left['cpu']}",
        f"- `{second}`: {right['recorded']} at {right['commit']}, {right['cpu']}",
        "",
        "| Benchmark | {first} | {second} | delta | std dev | verdict |".format(first=first, second=second),
        "|:--|--:|--:|--:|:--|:--|",
    ]
    for name in sorted(cases):
        before = next((case for case in left["cases"] if case["name"] == name), None)
        after = next((case for case in right["cases"] if case["name"] == name), None)
        if before is None or after is None:
            side = "only in the newer record" if before is None else "only in the older record"
            lines.append(
                f"| {name} | {duration(before['mean_ns']) if before else '—'} | "
                f"{duration(after['mean_ns']) if after else '—'} | | | {side} |"
            )
            continue
        mean_before, mean_after = before["mean_ns"], after["mean_ns"]
        if mean_before <= 0:
            continue
        delta = (mean_after - mean_before) / mean_before * 100
        verdict = "≈ noise" if abs(delta) < NOISE_PERCENT else ("slower" if delta > 0 else "faster")
        noise = " / ".join(
            f"{case['std_dev_ns'] / case['mean_ns'] * 100:.1f}%" if case["mean_ns"] else "—"
            for case in (before, after)
        )
        lines.append(
            f"| {name} | {duration(mean_before)} | {duration(mean_after)} | "
            f"{delta:+.1f}% | {noise} | {verdict} |"
        )
    lines += [
        "",
        f"A delta under {NOISE_PERCENT:.0f}% reads as noise on a shared machine.",
        "The std dev column is each side's own spread, older first: a delta smaller than it is not a result.",
        "A regression beyond that repeats on a second run, or it is noise.",
    ]
    REPORTS.mkdir(exist_ok=True)
    out = REPORTS / f"{first}-vs-{second}.md"
    out.write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print(f"\nreport: written to {out.relative_to(HERE.parent)}")
    return 0


def comparability(left: dict[str, object], right: dict[str, object]) -> list[str]:
    """What makes a delta dishonest: another machine, another knob, another harness, a dirty tree."""
    warnings = []
    if left["cpu"] != right["cpu"] or left["platform"] != right["platform"]:
        warnings.append("the two records come from different machines: absolute numbers do not carry over")
    if left["parameters"] != right["parameters"]:
        warnings.append(
            f"the parameters differ ({left['parameters']} vs {right['parameters']}): "
            "the two runs did not cover the same workloads"
        )
    elif "ladder" in left["parameters"]:
        warnings.append("neither record names the workloads it covered: the cases carry no dataset tag")
    if left["criterion"] != right["criterion"]:
        warnings.append(f"criterion changed ({left['criterion']} vs {right['criterion']}): the harness moved")
    if left["rustc"] != right["rustc"]:
        warnings.append(f"the toolchain changed ({left['rustc']} vs {right['rustc']})")
    if right["dirty"]:
        warnings.append(f"the newer record was taken on a dirty tree at {right['commit']}")
    return warnings


def usage() -> None:
    """The two modes, on stderr, with the exit code misuse deserves."""
    print(
        "usage: report.py collect <tag>\n"
        "       report.py compare <a> <b>\n"
        "  collect reads the criterion run saved under <tag> into reports/<tag>.{json,md}\n"
        "  compare reads two records back into reports/<a>-vs-<b>.md",
        file=sys.stderr,
    )
    raise SystemExit(2)


def main(argv: list[str]) -> int:
    """Dispatch the two modes."""
    if len(argv) == 3 and argv[1] == "collect":
        return collect(argv[2])
    if len(argv) == 4 and argv[1] == "compare":
        return compare(argv[2], argv[3])
    usage()
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
