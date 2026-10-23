# Bench

The performance harness of the repository: criterion benches over the sync family and the real CLI, a deterministic tree generator, and a report pipeline.
Fully opt-in and manual: nothing here runs in CI, nothing here is part of the gate.

Criterion is the harness, not hyperfine, and for a reason.
Hyperfine times a process from the outside: it can say how long `nxr up` took, but it cannot set the experiment up.
Every case here needs a freshly generated tree, a freshly seeded in-memory mock on a free port, a half-populated remote or a dribbling server, per iteration.
That setup is what makes the numbers mean something, and it has to live in the same process as the clock.
Criterion also brings the statistics, the throughput accounting and a machine-readable `estimates.json` per case, which is what the reports are built from.

The bytes come from `mock-nexus`, in process, never from docker.
The real-Nexus stand exists under `stand/real-nexus/` and its battery runs nightly, and benchmarks want the opposite of that: hermetic, repeatable, no JVM startup, no image pull, no registry.
It also wants failure injection, which the mock has and a real server does not.
The price is honesty about the absolute numbers: loopback HTTP with an in-memory store measures the client, and "the client" is exactly what this repository ships.
Read the numbers as a floor and as a relative signal, never as a prediction of a real Nexus.

## Scenarios

| Target | What it exercises | Throughput counted as |
|:-------|:------------------|:----------------------|
| `scan` | the local walk (`Nxr::scan`), the front door of every plan | files |
| `diff` | the delta report (`Nxr::delta`): local scan, remote probes over HEAD, classify | files |
| `up` | the full put flow against an empty mock: bytes, then the marker of the same name | bytes |
| `down` | the full get flow with sibling digest checks | bytes |
| `down-slow` | the same download against a server that dribbles every body in delayed chunks | bytes |
| `cli-up`, `cli-down` | the real `nxr` binary against the same mock: spawn, argv, plan, transfer, render | bytes |

The library targets call `Nxr` directly, so they measure the core and nothing above it.
The `cli` targets drive the shipped binary, so the gap between `cli-up` and `up` at the same dataset and worker count is the CLI's own cost, process spawn included.

## The ladders

Both dimensions are the environment's, so a curve costs no rebuild:

| Variable | Default | Meaning |
|:---------|:--------|:--------|
| `NXR_BENCH_FILES` | `100,1000` | the file counts to cover, comma separated |
| `NXR_BENCH_WORKERS` | `1,8` | the worker counts to cover, comma separated |
| `NXR_BENCH_SIZE` | `2048` | bytes per file |
| `NXR_BENCH_DEPTH` | `4` | maximum directory depth |
| `NXR_BENCH_FANOUT` | `4` | directory fan-out per level |
| `NXR_BENCH_SEED` | `42` | content seed |
| `NXR_BENCH_FSYNC` | unset | `1` flushes every generated file before the clock. Use it on the 10k rung |

A case is a rung of each ladder: `up/100f-4d-4w-2048b-seed42/8` is a hundred 2 KiB files at eight workers.
The content is a SplitMix64 stream, so the same parameters build the same bytes on every machine and every run.
The setup is untimed, but its writeback is not: on a large tree the dirty pages can land inside the timed transfer.
Set `NXR_BENCH_FSYNC=1` to move that flush into setup. The 10k rung is not a client verdict without it.
The default pair keeps a full run around a few minutes, and the heavy shapes are one variable away:

```bash
NXR_BENCH_FILES=100,1000,10000 NXR_BENCH_WORKERS=1,4,8,32 just bench up
```

## Run

```bash
just bench                    # every target, the default ladders
just bench scan               # one target: scan, diff, up, down or cli
just bench scan --sample-size 10 --measurement-time 3
```

Anything after the target name is a criterion flag, and the recipe supplies the `--` cargo needs before it.
A bare word that is not one of the five targets is refused outright, because cargo treats an unknown word as a test filter and exits 0 having run nothing.

`just bench` also builds the release `nxr` for the `cli` targets and passes its path through `NXR_BIN`.
A single-target run that cannot reach them (`just bench scan`) skips that build.
Running the `cli` target by hand means building the binary first:

```bash
cargo build --release -p nexus-raw
cd bench && NXR_BIN=../target/release/nxr cargo bench --bench cli
```

The benches write their trees and download targets under `bench/target/bench-trees/`, so a run leaves no trace in the repository, and criterion writes under `bench/target/criterion/`.
Both die with `cargo clean`, which is why a run worth keeping becomes a report.

## What the clock covers

The rules are the same in every target, and they are the reason a number here is worth quoting:

- **Setup is untimed.** The tree is generated, the mock is started and seeded, and the target directory is wiped before the clock starts.
- **Teardown is untimed.** The mock is dropped after the attempt returns, never inside the measurement.
- **A failed attempt is not a sample.** The client stack loses a request every few tens of thousands on loopback while the server stays healthy, and such an attempt is retried outside the clock and costs no iteration, so a hiccup cannot inflate a mean.
  Four failed attempts in a row panic the run: an attempt that never succeeds is a broken setup, not noise.
- **Both sides start fresh where the tool mutates them.** `up` writes the missing local siblings into the source tree and a file that already carries one is not hashed again, so the `up` and `cli-up` setups rebuild the tree every iteration.
  A reused tree would measure the marker-present path, and a reused remote the skip path.
- **The configuration is the shipped one.** Retry 4, connect timeout 15 s and stall 30 s, exactly what `--retry`, `--connect-timeout-secs` and `--stall-secs` default to, and only the worker count is a dimension.

## Reports

A criterion baseline is a working surface, not a record: it lives under `target/` and `cargo clean` takes it.
So a run worth keeping is saved under a tag and turned into two files under `bench/reports/`:

```bash
just bench-report before                          # the suite, saved as `before`
# ... make the change ...
just bench-report after
just bench-compare before after                   # reports/before-vs-after.md, plus a summary
```

| File | What it holds |
|:-----|:--------------|
| `reports/<tag>.json` | every case with mean, median, standard deviation and slope in nanoseconds, plus the commit, toolchain, machine, CPU and the ladder variables |
| `reports/<tag>.md` | the same numbers as a table, with the leaders and the ladder scaling worked out |
| `reports/<a>-vs-<b>.md` | the delta table: each case on both sides, the delta, the spread on each side and a verdict |

The comparison warns before it counts: two records from different machines, different ladders, different criterion versions or different toolchains do not make a delta, and the file says so at the top.
A delta under 5% reads as noise, and a delta smaller than the spread printed next to it is not a result.
A regression beyond that repeats on a second run, or it is noise.

Reports are committed on demand, never automatically.
They are the memory of the project's performance decisions.

## What is not measured

- **The retry and rate-limit machinery.** The core retries, backs off and halves its worker pool on a 429, and no target here measures that cost: `down-slow` covers a slow server, not a refusing one.
  The conformance suite pins what those paths do, and their price is open.
- **Real network conditions.** No TLS, no RTT beyond loopback, no proxy, no disk-backed server.
- **Cold caches.** The trees are generated immediately before they are read, so the page cache is warm, and `up` reads files it just wrote.
- **Memory and allocation.** Criterion times wall clock, and nothing here counts bytes allocated or resident.

## Isolation from the gate

The mechanism is workspace exclusion, not `.gitignore` and not a cargo feature:

- the root `Cargo.toml` lists `bench` in `exclude`, so every root cargo command (`test --workspace`, `clippy --workspace`, `fmt --all`, `cargo metadata`) cannot see the package.
- `bench/Cargo.toml` declares its own `[workspace]`, so it never tries to join the repo workspace from the other side.
- the bench workspace keeps its own `Cargo.lock` and its own `target/`.
- `just check` runs the gate over the repo workspace only: bench code is neither compiled nor linted there.

Two things keep the isolation from rotting:

- `just lock` and `just version` resolve the bench lock along with the two example locks, so the release bump moves it too.
- `scripts/version-check.sh` asserts the bench lock records the workspace version, because `just bench` runs `cargo bench --locked` and a stale lock stops a run before it measures anything.

What the gate still touches: the prek hygiene hooks (`trailing-whitespace`, `end-of-file-fixer`, `check-toml`, `check-md-semicolon`, `check-external-names`, `shellcheck`) scan every tracked file, this directory included.
They are per-file and cost milliseconds.

## The tree generator

The benches generate their trees themselves.
The `gen-tree` binary builds the same tree by hand, for inspection, for the mock server stand, or for sizing an experiment:

```bash
cargo run --manifest-path bench/Cargo.toml --bin gen-tree -- \
    --out /tmp/nxr-tree --files 500 --depth 4 --fanout 4 --size 4096 --seed 42
```

Flags: `--out` (required), `--files`, `--depth`, `--fanout`, `--size`, `--seed`.
Both the benches and the binary share the tree module in `bench/src/lib.rs`, so a hand-generated tree and a bench tree are byte-identical for the same parameters.

## Layout

| Path | Role |
|:-----|:-----|
| `src/lib.rs` | the shared harness: the tree spec and its generator, the ladders, the in-process mock, the measured-attempt loop, the CLI runner |
| `src/bin/gen-tree.rs` | the tree generator binary |
| `benches/scan.rs`, `benches/diff.rs`, `benches/up.rs`, `benches/down.rs`, `benches/cli.rs` | one criterion target per scenario |
| `report.py` | the report reader: collects a saved run into a record, compares two records |
| `reports/<tag>.json`, `reports/<tag>.md` | the committed records, created by the first `just bench-report` |
