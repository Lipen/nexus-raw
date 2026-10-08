//! `diff`: the delta report, a local tree against a half-populated enumeration.
//! Half the names exist on both sides, the rest only locally: adds and sames in one union.

use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use nexus_raw_bench::{runtime, write_tree, Mock, TreeSpec};
use nexus_raw_core::{ArtifactName, Enumeration};

fn bench_diff(c: &mut Criterion) {
    let rt = runtime();
    let spec = TreeSpec::standard();
    let dir = write_tree(spec).expect("the generated tree writes");
    let mock = Mock::start();
    let half: Vec<(u32, ArtifactName)> = (0..spec.files)
        .filter(|i| i % 2 == 0)
        .map(|i| {
            let name = ArtifactName::parse(&spec.file_name(i)).expect("generated names fit");
            (i, name)
        })
        .collect();
    for (i, name) in &half {
        mock.seed(name.as_str(), &spec.file_bytes(*i));
    }
    let enum_names: Vec<ArtifactName> = half.into_iter().map(|(_, name)| name).collect();

    let mut group = c.benchmark_group("diff");
    group.throughput(Throughput::Elements(u64::from(spec.files)));
    group.bench_function("standard", |b| {
        // The enumeration is the input, built outside the timing: the measured
        // work is the report, not a clone of a hundred names.
        b.iter_batched(
            || Enumeration::Names(enum_names.clone()),
            |enum_src| {
                let mut report = match rt.block_on(mock.nxr.delta(&dir, enum_src)) {
                    Ok(r) => r,
                    Err(first) => {
                        // See the up bench: loopback storms are transient, retry on a fresh pool.
                        eprintln!("bench: diff hit a transport error, retrying: {first}");
                        let mut last = first;
                        let mut out = None;
                        for attempt in 0..3 {
                            std::thread::sleep(Duration::from_millis(250));
                            match rt.block_on(
                                mock.facade()
                                    .delta(&dir, Enumeration::Names(enum_names.clone())),
                            ) {
                                Ok(r) => {
                                    out = Some(r);
                                    break;
                                }
                                Err(e) => {
                                    last = e;
                                    eprintln!("bench: diff retry {attempt} failed: {last}");
                                }
                            }
                        }
                        match out {
                            Some(r) => r,
                            None => panic!("diff kept failing after retries: {last}"),
                        }
                    }
                };
                assert_eq!(
                    report.len(),
                    spec.files as usize,
                    "the delta union lost names"
                );
                black_box(std::mem::take(&mut report))
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20).warm_up_time(Duration::from_secs(1));
    targets = bench_diff
}
criterion_main!(benches);
