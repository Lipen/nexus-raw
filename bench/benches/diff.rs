//! `diff`: the delta report, a local tree against a half-populated enumeration.
//! Half the names exist on both sides, the rest only locally: adds and sames in one union.
//!
//! The remote side is read-only, so one seeded mock serves every iteration: the
//! per-iteration cost is a fresh connection pool, which is what the probes need.

use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_raw_bench::{matrix, measure, runtime, worker_ladder, write_tree, Mock};
use nexus_raw_core::{ArtifactName, Enumeration};

fn bench_diff(c: &mut Criterion) {
    let rt = runtime();
    let mut group = c.benchmark_group("diff");
    for spec in matrix() {
        let dir = write_tree(spec).expect("the generated tree writes");
        let names = spec.names();
        let mock = Mock::start(8);
        for (index, name) in names.iter().enumerate() {
            if index % 2 == 0 {
                let i = u32::try_from(index).expect("the index fits u32");
                mock.seed(name.as_str(), &spec.file_bytes(i));
            }
        }
        let half: Vec<ArtifactName> = names.iter().step_by(2).cloned().collect();
        for workers in worker_ladder() {
            group.throughput(Throughput::Elements(u64::from(spec.files)));
            group.bench_with_input(BenchmarkId::new(spec.tag(), workers), &dir, |b, dir| {
                b.iter_custom(|iters| {
                    measure(
                        iters,
                        || (mock.facade(workers), Enumeration::Names(half.clone())),
                        |(nxr, src)| rt.block_on(nxr.delta(dir, src)).map_err(|e| e.to_string()),
                        |report| {
                            assert_eq!(
                                report.len(),
                                spec.files as usize,
                                "the delta union lost names"
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
    targets = bench_diff
}
criterion_main!(benches);
