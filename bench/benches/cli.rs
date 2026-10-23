//! `cli`: the end-to-end path a user runs.
//!
//! Every other target times the library. This one drives the real `nxr` binary
//! against the same mock, so the clock covers what a pipeline feels: process
//! spawn, argv parsing, the config, the plan, the transfer, the rendering.
//! The gap between a `cli-up` number and its `up` number is the CLI's own cost.
//!
//! `just bench` builds the release binary and sets `NXR_BIN`.
//! A bare `cargo bench` finds it under `target/` or fails with the instructions.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_raw_bench::{
    down_target, matrix, measure, names_path, nxr_bin, run_nxr, wipe, worker_ladder, write_names,
    write_tree, Mock,
};

fn bench_cli_up(c: &mut Criterion) {
    let bin = nxr_bin();
    eprintln!("bench: cli-up drives {}", bin.display());
    let mut group = c.benchmark_group("cli-up");
    for spec in matrix() {
        for workers in worker_ladder() {
            let workers_arg = workers.to_string();
            group.throughput(Throughput::Bytes(spec.bytes()));
            group.bench_function(BenchmarkId::new(spec.tag(), workers), |b| {
                b.iter_custom(|iters| {
                    measure(
                        iters,
                        || {
                            // As in the `up` bench: the source tree is rebuilt every
                            // iteration, because `up` writes its siblings into it.
                            let dir = write_tree(spec).expect("the generated tree writes");
                            (dir, Mock::start(workers))
                        },
                        |(dir, mock)| {
                            let dir_arg = dir.to_string_lossy().to_string();
                            run_nxr(&[
                                "up",
                                &dir_arg,
                                &mock.dir,
                                "--workers",
                                &workers_arg,
                                "--quiet",
                            ])
                        },
                        |()| {},
                    )
                })
            });
        }
    }
    group.finish();
}

fn bench_cli_down(c: &mut Criterion) {
    let bin = nxr_bin();
    eprintln!("bench: cli-down drives {}", bin.display());
    let mut group = c.benchmark_group("cli-down");
    for spec in matrix() {
        let target = down_target(spec);
        let target_arg = target.to_string_lossy().to_string();
        let names = names_path(spec);
        write_names(&names, spec).expect("the names file writes");
        let names_arg = names.to_string_lossy().to_string();
        for workers in worker_ladder() {
            let workers_arg = workers.to_string();
            group.throughput(Throughput::Bytes(spec.bytes()));
            group.bench_function(BenchmarkId::new(spec.tag(), workers), |b| {
                b.iter_custom(|iters| {
                    measure(
                        iters,
                        || {
                            let mock = Mock::start(workers);
                            mock.seed_tree(spec);
                            wipe(&target);
                            mock
                        },
                        |mock| {
                            run_nxr(&[
                                "down",
                                &mock.dir,
                                &target_arg,
                                "--names-from",
                                &names_arg,
                                "--workers",
                                &workers_arg,
                                "--quiet",
                            ])
                        },
                        |()| {},
                    )
                })
            });
        }
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1));
    targets = bench_cli_up, bench_cli_down
}
criterion_main!(benches);
