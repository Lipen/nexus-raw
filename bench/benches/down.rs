//! `down`: the full get flow with sibling digest checks into an empty directory.
//! The mock is pre-seeded with bytes and real `.sha256` siblings: every name verifies.
//!
//! The source tree is never involved: the artifacts come out of the mock's memory
//! and land in a target directory that every iteration wipes, so no run inherits
//! a `.part` file or a sibling from the run before it.
//!
//! Two groups: the straight-through server, and a dribbling one.
//! The second answers what the first cannot: whether the pool keeps the pipe full
//! when every response body arrives in pieces.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_raw_bench::{down_target, matrix, measure, runtime, slow_server, wipe, worker_ladder, Mock};
use nexus_raw_core::{Enumeration, Summary};

fn bench_down(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("down");
    for spec in matrix() {
        let target = down_target(spec);
        for workers in worker_ladder() {
            group.throughput(Throughput::Bytes(spec.bytes()));
            group.bench_function(BenchmarkId::new(spec.tag(), workers), |b| {
                b.iter_custom(|iters| {
                    measure(
                        iters,
                        || {
                            let mock = Mock::start(workers);
                            mock.seed_tree(spec);
                            wipe(&target);
                            (mock, Enumeration::Names(spec.names()))
                        },
                        |(mock, src)| {
                            rt.block_on(mock.nxr.down(&target, src, true))
                                .map_err(|e| e.to_string())
                        },
                        |summary: &Summary| {
                            assert!(
                                summary.failed.is_empty()
                                    && summary.downloaded + summary.skipped == spec.files as usize,
                                "the download lost or doubled names: {summary:?}"
                            );
                        },
                    )
                })
            });
        }
    }
    group.finish();
}

fn bench_down_slow(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("down-slow");
    // A dribbling server multiplies the dataset cost by its chunk count, so this
    // group runs the smallest rung of the ladder only, with fewer samples.
    let Some(spec) = matrix().into_iter().next() else {
        return;
    };
    let target = down_target(spec);
    group.sample_size(10);
    for workers in worker_ladder() {
        group.throughput(Throughput::Bytes(spec.bytes()));
        group.bench_function(BenchmarkId::new(spec.tag(), workers), |b| {
            b.iter_custom(|iters| {
                measure(
                    iters,
                    || {
                        let mock = Mock::start_scenario(workers, slow_server());
                        mock.seed_tree(spec);
                        wipe(&target);
                        (mock, Enumeration::Names(spec.names()))
                    },
                    |(mock, src)| {
                        rt.block_on(mock.nxr.down(&target, src, true))
                            .map_err(|e| e.to_string())
                    },
                    |summary: &Summary| {
                        assert!(
                            summary.failed.is_empty()
                                && summary.downloaded + summary.skipped == spec.files as usize,
                            "the slow download lost or doubled names: {summary:?}"
                        );
                    },
                )
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1));
    targets = bench_down, bench_down_slow
}
criterion_main!(benches);
