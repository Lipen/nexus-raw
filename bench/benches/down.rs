//! `down`: the full get flow with sibling digest checks into an empty directory.
//! The mock is pre-seeded with bytes and real `.sha256` siblings: every name verifies.

use std::path::Path;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use nexus_raw_bench::{down_target, runtime, Mock, TreeSpec};
use nexus_raw_core::{Enumeration, Error, Summary};
use tokio::runtime::Runtime;

// See the up bench: fewer samples, less loopback exposure, faster hand runs.
const SAMPLES: usize = 20;

fn retry_down(
    rt: &Runtime,
    mock: &Mock,
    target: &Path,
    spec: TreeSpec,
    mut last: Error,
) -> Summary {
    for attempt in 0..3 {
        std::thread::sleep(Duration::from_millis(250));
        match rt.block_on(
            mock.facade()
                .down(target, Enumeration::Names(spec.names()), true),
        ) {
            Ok(s) => return s,
            Err(e) => {
                last = e;
                eprintln!("bench: down retry {attempt} failed: {last}");
            }
        }
    }
    panic!("down kept failing after retries: {last}");
}

fn bench_down(c: &mut Criterion) {
    let rt = runtime();
    let spec = TreeSpec::standard();
    let target = down_target(spec);
    let mut group = c.benchmark_group("down");
    group.throughput(Throughput::Bytes(
        u64::from(spec.files) * u64::from(spec.size),
    ));
    group.bench_function("standard", |b| {
        b.iter_batched(
            || {
                // A fresh mock holding every artifact, and an empty target directory.
                let mock = Mock::start();
                for i in 0..spec.files {
                    mock.seed(&spec.file_name(i), &spec.file_bytes(i));
                }
                let _ = std::fs::remove_dir_all(&target);
                std::fs::create_dir_all(&target).expect("the target directory is creatable");
                mock
            },
            |mock| {
                let summary = match rt.block_on(mock.nxr.down(
                    &target,
                    Enumeration::Names(spec.names()),
                    true,
                )) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("bench: down hit a transport error, retrying: {e}");
                        retry_down(&rt, &mock, &target, spec, e)
                    }
                };
                assert!(
                    summary.failed.is_empty()
                        && summary.downloaded + summary.skipped >= spec.files as usize,
                    "the download skipped work: {summary:?}"
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
    targets = bench_down
}
criterion_main!(benches);
