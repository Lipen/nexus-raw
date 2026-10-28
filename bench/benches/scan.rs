//! `scan`: the local walk behind every plan.
//! `up`, `diff` and `delta` all start from it, so its cost rides on every flow.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use nexus_raw_bench::{config, matrix, write_tree};
use nexus_raw_core::Nxr;
use tokio::sync::mpsc;

fn bench_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan");
    for spec in matrix() {
        let dir = write_tree(spec).expect("the generated tree writes");
        // The scan never touches the network: the base URL only has to parse,
        // and the worker count is irrelevant to a local walk.
        let (tx, _rx) = mpsc::unbounded_channel();
        let nxr = Nxr::new(config("http://127.0.0.1:9/".to_owned(), 8), tx).expect("a valid base");
        group.throughput(Throughput::Elements(u64::from(spec.files)));
        group.bench_with_input(BenchmarkId::from_parameter(spec.tag()), &dir, |b, dir| {
            b.iter(|| {
                black_box(
                    nxr.scan(black_box(dir))
                        .expect("the generated tree is legal"),
                )
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1));
    targets = bench_scan
}
criterion_main!(benches);
