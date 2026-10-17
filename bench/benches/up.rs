//! `up`: the full put flow with markers against an empty mock.
//! Every name plans as an add: bytes land, then the marker of the same name.
//!
//! Both sides start fresh every iteration, and both for the same reason: the
//! source tree is what `up` reads *and* writes, the remote is what it fills.
//! `up` gives every markerless local file its sibling, and a local file that
//! already carries one is not hashed again, so a reused tree would measure the
//! marker-present path from the second iteration on.
//! A reused remote would measure the skip path from the second iteration on.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_raw_bench::{matrix, measure, runtime, worker_ladder, write_tree, Mock};
use nexus_raw_core::Summary;

fn bench_up(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("up");
    for spec in matrix() {
        for workers in worker_ladder() {
            group.throughput(Throughput::Bytes(spec.bytes()));
            group.bench_function(BenchmarkId::new(spec.tag(), workers), |b| {
                b.iter_custom(|iters| {
                    measure(
                        iters,
                        || {
                            let dir = write_tree(spec).expect("the generated tree writes");
                            (dir, Mock::start(workers))
                        },
                        |(dir, mock)| {
                            rt.block_on(mock.nxr.up(&dir, None, true, None))
                                .map_err(|e| e.to_string())
                        },
                        |summary: &Summary| {
                            assert!(
                                summary.failed.is_empty()
                                    && summary.uploaded + summary.skipped == spec.files as usize,
                                "the upload lost or doubled names: {summary:?}"
                            );
                        },
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
    targets = bench_up
}
criterion_main!(benches);
