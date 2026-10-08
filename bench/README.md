# Bench

The performance harness of the repository: criterion benches for the sync family, a deterministic tree generator and a report archive.
Fully opt-in and manual: nothing here runs in CI, nothing here is part of the gate.

The bench workspace is standalone on purpose.
Its `Cargo.toml` carries its own `[workspace]`, and the repo root excludes `bench/`, so `cargo test --workspace`, `just check`, clippy and fmt never see it.
It has its own `Cargo.lock` and its own `target/`.

## When to bench

Before and after any change that touches scheduling or transfer shape:

- the worker semaphore and its fan-out,
- the retry and stall machinery,
- the classify/plan stage,
- the transport client build,
- anything that changes how many requests a flow issues.

Numbers from localhost mean nothing in absolute terms: no TLS, no real Nexus latency, a std-only mock.
They mean everything in relative terms: same machine, same tree, same code shape around the change.

## Scenarios

| Benches | What it exercises |
|:--------|:------------------|
| `scan` | the local walk (`Nxr::scan`), the front door of every plan; two tree sizes |
| `diff` | the delta report (`Nxr::delta`): local scan, remote probes over HEAD, classify |
| `up` | the full put flow against an empty mock: bytes, then markers of the same name |
| `down` | the full get flow with sibling digest checks: bytes plus `.sha256` verify |

The default tree everywhere: 200 files, depth 4, fanout 4, 2 KiB per file, seed 42.
The content is a SplitMix64 stream, so the same parameters build the same bytes on every machine.

## Run

```bash
just bench              # every scenario, default tree, a couple of minutes
just bench scan         # one scenario: scan, diff, up or down
```

Anything else passes straight to cargo:

```bash
just bench -- --save-baseline before
just bench -- --baseline before
just bench up -- --save-baseline before
```

Raw form, same thing:

```bash
cd bench && cargo bench
```

The benches build their trees under `bench/target/bench-trees/` as untimed setup, so a run leaves no trace in the repository.
Criterion stores its measurements under `bench/target/criterion/`: baselines live there too, and `cargo clean` takes them.
That is fine: the durable record is a report (below), not a baseline.

## Baseline workflow

Around any scheduler-touching change:

```bash
# on the base commit
just bench -- --save-baseline before

# make the change

# compare against the saved baseline
just bench -- --baseline before
```

Criterion prints the change per benchmark with its noise threshold.
A regression beyond noise repeats on a second run, or it is noise.

Two noise sources are worth naming:

- This is a shared machine: a busy box shifts every number.
  Read relative changes on the same machine, close in time, never absolutes.
- A long run opens tens of thousands of short-lived loopback connections, and the
  client stack loses one every few tens of thousands in brief storms (`error sending request`
  while the server stays healthy and answers the next request).
  The core retries absorb most of it, and the benches retry an iteration a few more
  times on a fresh connection pool.
  A `bench: ... retrying` line in the output marks such a sample: it inflates that
  one number, so re-run before trusting a slowdown that a retry line accompanies.

To keep both sides for a report, run the after side with `--save-baseline after` and read the two numbers from the criterion output (or from `bench/target/criterion/<bench>/<case>/new/estimates.json` and `.../base/`).

## The tree generator

The benches generate their trees themselves.
The `gen-tree` binary builds the same tree by hand, for inspection, for the mock server stand, or for sizing an experiment:

```bash
cargo run --manifest-path bench/Cargo.toml --bin gen-tree -- \
    --out /tmp/nxr-tree --files 500 --depth 4 --fanout 4 --size 4096 --seed 42
```

Flags: `--out` (required), `--files`, `--depth`, `--fanout`, `--size`, `--seed`.
Both the benches and the binary share the tree module in `bench/src/lib.rs`, so a hand-generated tree and a bench tree are byte-identical for the same parameters.

## Reports

A bench run without a written report evaporates with `cargo clean`.
So: when a run matters, write it up.

1. Copy `reports/TEMPLATE.md` to `reports/YYYY-MM-DD-<topic>.md`.
2. Fill in machine, tree shape, the before/after table and the one-line verdict.
3. Commit it.

Reports are committed on demand, never automatically, never in CI.
They are the memory of the project's performance decisions.

## Gate isolation

The mechanism is workspace exclusion, not `.gitignore` and not a cargo feature:

- the root `Cargo.toml` lists `bench` in `exclude`, so every root cargo command (`test --workspace`, `clippy --workspace`, `fmt --all`, `cargo metadata`) cannot see the package;
- `bench/Cargo.toml` declares its own `[workspace]`, so it never tries to join the repo workspace from the other side;
- the bench workspace keeps its own `Cargo.lock`, its own `target/` and its own resolution of criterion and friends;
- `just check` runs the gate over the repo workspace only: bench code is neither compiled nor linted there.

What the gate does still touch: the prek hygiene hooks (`trailing-whitespace`, `end-of-file-fixer`, `check-toml`) scan every tracked file, including the markdown and the manifest here.
They are per-file and cost milliseconds; they are the only gate surface this directory adds.

## Layout

| Path | Role |
|:-----|:-----|
| `src/lib.rs` | the shared harness: tree spec and generator, mock bootstrap, config, runtime |
| `src/bin/gen-tree.rs` | the tree generator binary |
| `benches/scan.rs`, `benches/diff.rs`, `benches/up.rs`, `benches/down.rs` | one criterion target per scenario |
| `reports/TEMPLATE.md` | the report template |
| `reports/YYYY-MM-DD-<topic>.md` | committed reports, on demand |
