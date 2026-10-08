//! `up`: the full put flow with markers against an empty mock.
//! Every name plans as an add: bytes land, then the marker of the same name.

use std::path::Path;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use nexus_raw_bench::{runtime, write_tree, Mock, TreeSpec};
use nexus_raw_core::{Error, Summary};
use tokio::runtime::Runtime;

// A long run opens tens of thousands of short-lived loopback connections, and the
// client stack loses one every few tens of thousands in brief storms (the server
// stays healthy and answers the next request). The core retries absorb most of it;
// the retry helper below rides out the rest. Fewer samples keep the exposure (and
// the hand-run time) small; the relative before/after reading does not need 100.
const SAMPLES: usize = 20;

fn retry_up(rt: &Runtime, mock: &Mock, dir: &Path, mut last: Error) -> Summary {
    for attempt in 0..3 {
        std::thread::sleep(Duration::from_millis(250));
        match rt.block_on(mock.facade().up(dir, None, true, None)) {
            Ok(s) => return s,
            Err(e) => {
                last = e;
                eprintln!("bench: up retry {attempt} failed: {last}");
            }
        }
    }
    panic!("up kept failing after retries: {last}");
}

fn bench_up(c: &mut Criterion) {
    let rt = runtime();
    let spec = TreeSpec::standard();
    let mut group = c.benchmark_group("up");
    group.throughput(Throughput::Bytes(
        u64::from(spec.files) * u64::from(spec.size),
    ));
    group.bench_function("standard", |b| {
        b.iter_batched(
            || {
                // A fresh tree (no leftover markers) and an empty remote per iteration:
                // the setup is untimed, the transfer is what gets measured.
                let dir = write_tree(spec).expect("the generated tree writes");
                (dir, Mock::start())
            },
            |(dir, mock)| {
                let summary = match rt.block_on(mock.nxr.up(&dir, None, true, None)) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("bench: up hit a transport error, retrying: {e}");
                        retry_up(&rt, &mock, &dir, e)
                    }
                };
                assert!(
                    summary.failed.is_empty()
                        && summary.uploaded + summary.skipped == spec.files as usize,
                    "the upload lost or doubled names: {summary:?}"
                );
            },
            BatchSize::PerIteration,
        )
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(SAMPLES).warm_up_time(Duration::from_secs(1));
    targets = bench_up
}
criterion_main!(benches);
