//! Shared parts of the bench workspace: the deterministic tree, the in-process
//! mock and the measured-attempt loop.
//!
//! The benches and the `gen-tree` binary both go through this module, so a tree
//! a human generates by hand and a tree a bench builds are the same bytes.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{Duration, Instant};

use nexus_raw_core::model::sibling;
use nexus_raw_core::{ArtifactName, Config, Digest, Event, Nxr};
use tokio::sync::mpsc;

/// The synthetic tree shape.
/// Every parameter is explicit: no environment, no defaults hiding in code paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeSpec {
    /// File count.
    pub files: u32,
    /// Maximum directory depth (1 keeps every file in the root).
    pub depth: u32,
    /// Directory fan-out per level step.
    pub fanout: u32,
    /// Bytes per file.
    pub size: u32,
    /// Content seed: the same seed builds the same bytes on every machine.
    pub seed: u64,
}

impl TreeSpec {
    /// The default shape: 200 files, 4 levels of 4, 2 KiB each.
    #[must_use]
    pub fn standard() -> Self {
        Self {
            files: 200,
            depth: 4,
            fanout: 4,
            size: 2048,
            seed: 42,
        }
    }

    /// The same shape with another file count: the dataset ladder is built this way.
    #[must_use]
    pub fn with_files(self, files: u32) -> Self {
        Self { files, ..self }
    }

    /// The payload of one pass over the tree, in bytes: what `Throughput::Bytes` counts.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        u64::from(self.files) * u64::from(self.size)
    }

    /// A stable tag for this shape, used as the tree directory name.
    /// The report reader parses this string back into files and size: keep the shape.
    #[must_use]
    pub fn tag(&self) -> String {
        format!(
            "{}f-{}d-{}w-{}b-seed{}",
            self.files, self.depth, self.fanout, self.size, self.seed
        )
    }

    /// The relative name of file `i`: fanout-base digits for the directories, then the file.
    #[must_use]
    pub fn file_name(&self, i: u32) -> String {
        let depth = self.depth.max(1);
        let fanout = u64::from(self.fanout.max(1));
        let level = i % depth;
        let mut segments = Vec::with_capacity(level as usize + 1);
        let mut k = u64::from(i / depth);
        for l in 1..=level {
            segments.push(format!("d{l}-{}", k % fanout));
            k /= fanout;
        }
        segments.push(format!("f{i:06}.bin"));
        segments.join("/")
    }

    /// The seeded content of file `i`: a SplitMix64 stream, 8 bytes per step.
    /// The same `(spec, i)` pair produces the same bytes on every machine and every run.
    #[must_use]
    pub fn file_bytes(&self, i: u32) -> Vec<u8> {
        let mut state = self.seed ^ u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut buf = vec![0_u8; usize::try_from(self.size).expect("file size fits memory")];
        for chunk in buf.chunks_mut(8) {
            let bytes = splitmix(&mut state).to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        buf
    }

    /// Every relative name of the tree, in index order.
    #[must_use]
    pub fn names(&self) -> Vec<ArtifactName> {
        (0..self.files)
            .map(|i| {
                ArtifactName::parse(&self.file_name(i)).expect("generated names fit the grammar")
            })
            .collect()
    }
}

/// The dataset ladder a run covers: one tree shape per file count.
///
/// `NXR_BENCH_FILES` is the ladder itself, as a comma-separated list of counts
/// (default `100,1000`).
/// `NXR_BENCH_SIZE`, `NXR_BENCH_DEPTH`, `NXR_BENCH_FANOUT` and `NXR_BENCH_SEED`
/// reshape every rung.
#[must_use]
pub fn matrix() -> Vec<TreeSpec> {
    let shape = TreeSpec {
        files: 0,
        size: number_env("NXR_BENCH_SIZE", 2048),
        depth: number_env("NXR_BENCH_DEPTH", 4),
        fanout: number_env("NXR_BENCH_FANOUT", 4),
        seed: number_env("NXR_BENCH_SEED", 42),
    };
    list_env("NXR_BENCH_FILES", &[100, 1000])
        .into_iter()
        .map(|files| shape.with_files(files))
        .collect()
}

/// The concurrency ladder for the transfer benches: `NXR_BENCH_WORKERS`, default `1,8`.
#[must_use]
pub fn worker_ladder() -> Vec<usize> {
    list_env("NXR_BENCH_WORKERS", &[1, 8])
}

/// A comma-separated list from the environment, or the default when unset.
/// A value that does not parse is misuse: it must stop the run, not fall back.
fn list_env<T: FromStr + Clone>(key: &str, default: &[T]) -> Vec<T> {
    match std::env::var(key) {
        Ok(raw) => raw
            .split(',')
            .map(|part| {
                part.trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("bench: {key}={raw} is not a comma-separated list"))
            })
            .collect(),
        Err(_) => default.to_vec(),
    }
}

/// One number from the environment, or the default when unset.
fn number_env<T: FromStr + Clone>(key: &str, default: T) -> T {
    list_env(key, std::slice::from_ref(&default))
        .into_iter()
        .next()
        .expect("a single default yields one value")
}

/// One SplitMix64 step: the whole generator is three multiplies, no dependencies.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Everything the benches write under, next to the criterion measurements.
#[must_use]
fn scratch() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench-trees")
}

/// The directory the benches build their trees in: under the bench target, never committed.
#[must_use]
pub fn tree_root(spec: TreeSpec) -> PathBuf {
    scratch().join(spec.tag())
}

/// The directory the `down` benches download into: next to the trees, wiped by its setup.
#[must_use]
pub fn down_target(spec: TreeSpec) -> PathBuf {
    scratch().join(format!("down-{}", spec.tag()))
}

/// The names file the end-to-end `down` bench hands to the CLI.
/// It sits beside the download target, which every iteration wipes.
#[must_use]
pub fn names_path(spec: TreeSpec) -> PathBuf {
    scratch().join(format!("names-{}.txt", spec.tag()))
}

/// (Re)build the tree at `root`: removed first, then written file by file.
/// A tree from an earlier run never leaks into this one: no stale markers, no stale bytes.
pub fn write_tree_at(root: &Path, spec: TreeSpec) -> std::io::Result<()> {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root)?;
    // The setup is untimed, but the OS writeback is not: on a large tree the
    // dirty pages land inside the timed transfer. `NXR_BENCH_FSYNC=1` moves
    // the flush into setup, at the price of one fdatasync per file.
    let fsync = std::env::var("NXR_BENCH_FSYNC").is_ok_and(|value| value == "1");
    for i in 0..spec.files {
        let path = root.join(spec.file_name(i));
        fs::create_dir_all(path.parent().expect("file names carry a parent"))?;
        let bytes = spec.file_bytes(i);
        if fsync {
            let mut file = fs::File::create(&path)?;
            file.write_all(&bytes)?;
            file.sync_data()?;
        } else {
            fs::write(path, bytes)?;
        }
    }
    Ok(())
}

/// (Re)build the canonical tree for `spec` under the bench target.
pub fn write_tree(spec: TreeSpec) -> std::io::Result<PathBuf> {
    let root = tree_root(spec);
    write_tree_at(&root, spec)?;
    Ok(root)
}

/// Wipe a directory a download iteration writes into, and recreate it empty.
pub fn wipe(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).expect("the target directory is creatable");
}

/// Write the names of `spec`, one per line: the enumeration the CLI `down` bench reads.
pub fn write_names(path: &Path, spec: TreeSpec) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for name in spec.names() {
        body.push_str(name.as_str());
        body.push('\n');
    }
    fs::write(path, body)
}

/// The bench configuration: localhost transport, no credentials, one worker count.
///
/// Everything except `workers` is the shipped default, so a number here answers
/// a question about the tool a user runs, not about a bench-only configuration:
/// `--retry 4`, `--connect-timeout-secs 15`, `--stall-secs 30`.
#[must_use]
pub fn config(base: String, workers: usize) -> Config {
    Config {
        base,
        tls_insecure: false,
        ca_bundle: None,
        workers,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(15),
        stall_timeout: Duration::from_secs(30),
        auth: None,
    }
}

/// A running mock with a facade pointed at its `bench/` directory.
/// Dropping it stops the server and frees the port.
pub struct Mock {
    /// The server handle: also the seeding surface.
    pub server: mock_nexus::MockNexus,
    /// The facade over the core.
    pub nxr: Nxr,
    /// The `bench/` directory URL the facade works on.
    pub dir: String,
    /// Keeps the event receiver alive: the facade writes progress into the channel.
    _events: mpsc::UnboundedReceiver<Event>,
}

impl Mock {
    /// Start the straight-through mock and a facade over its `bench/` directory.
    #[must_use]
    pub fn start(workers: usize) -> Self {
        Self::start_scenario(workers, mock_nexus::Scenario::Atomic)
    }

    /// Start a mock that plays `scenario`, its facade over the same `bench/` directory.
    #[must_use]
    pub fn start_scenario(workers: usize, scenario: mock_nexus::Scenario) -> Self {
        let server = mock_nexus::MockNexus::start(scenario).expect("the mock binds a free port");
        let dir = format!("{}bench/", server.base_url());
        let (tx, rx) = mpsc::unbounded_channel();
        let nxr = Nxr::new(config(dir.clone(), workers), tx).expect("the mock URL is a valid base");
        Self {
            server,
            nxr,
            dir,
            _events: rx,
        }
    }

    /// Store one artifact with its canonical marker: bytes first, marker after, as `up` lands them.
    pub fn seed(&self, name: &str, bytes: &[u8]) {
        let key = format!("bench/{name}");
        self.server.insert(&key, bytes);
        let marker = sibling::format_line(name, &Digest::of_bytes(bytes));
        self.server
            .insert(&format!("{key}.sha256"), marker.as_bytes());
    }

    /// Store the whole tree: every artifact with its marker.
    pub fn seed_tree(&self, spec: TreeSpec) {
        for i in 0..spec.files {
            self.seed(&spec.file_name(i), &spec.file_bytes(i));
        }
    }

    /// A second facade over the same server: a fresh connection pool, same directory.
    /// The event channel is dropped with the call: every core send site ignores a closed receiver.
    #[must_use]
    pub fn facade(&self, workers: usize) -> Nxr {
        let (tx, _rx) = mpsc::unbounded_channel();
        Nxr::new(config(self.dir.clone(), workers), tx).expect("the mock URL is a valid base")
    }
}

/// Run `iters` measured attempts and return the time the successful ones took.
///
/// The clock covers one attempt and nothing else: the setup is untimed, and an
/// attempt that returns `Err` is not a sample at all.
/// That is what lets a run survive loopback noise without inflating a number.
/// The client stack loses a request every few tens of thousands in brief storms
/// while the server stays healthy.
/// Here such an attempt costs no iteration, so the retry never lands inside a measurement.
/// A run of consecutive failures panics instead: an attempt that never succeeds
/// is a broken setup, not noise.
#[must_use]
pub fn measure<I, T, S, A, C>(iters: u64, mut setup: S, mut attempt: A, mut check: C) -> Duration
where
    S: FnMut() -> I,
    A: FnMut(I) -> Result<T, String>,
    C: FnMut(&T),
{
    /// How many attempts in a row may fail before the run gives up.
    const PATIENCE: u32 = 5;
    let mut total = Duration::ZERO;
    let mut measured = 0;
    let mut failed = 0;
    while measured < iters {
        let input = setup();
        let start = Instant::now();
        match attempt(input) {
            Ok(value) => {
                let elapsed = start.elapsed();
                check(&value);
                total += elapsed;
                measured += 1;
                failed = 0;
            }
            Err(why) => {
                failed += 1;
                assert!(
                    failed < PATIENCE,
                    "bench: {PATIENCE} attempts failed in a row, the last: {why}"
                );
                eprintln!("bench: not a sample, retrying outside the clock: {why}");
            }
        }
    }
    total
}

/// The `nxr` binary the end-to-end benches drive.
///
/// `NXR_BIN` wins.
/// Otherwise the release build of the workspace, then the debug one.
/// The panic names the fix: `just bench` builds the binary and sets the path.
#[must_use]
pub fn nxr_bin() -> PathBuf {
    if let Ok(path) = std::env::var("NXR_BIN") {
        return PathBuf::from(path);
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("bench/ sits inside the repository");
    let name = if cfg!(windows) { "nxr.exe" } else { "nxr" };
    for profile in ["release", "debug"] {
        let path = repo.join("target").join(profile).join(name);
        if path.is_file() {
            return path;
        }
    }
    panic!(
        "bench: no nxr binary under target/: run the cli benches through `just bench`, which builds one"
    );
}

/// One `nxr` invocation, the measured routine of the end-to-end benches.
///
/// The credentials are scrubbed from the environment: the mock asks for none,
/// and an inherited `NXR_AUTH` would turn a plain run into an authenticated one.
pub fn run_nxr(args: &[&str]) -> Result<(), String> {
    let bin = nxr_bin();
    let out = Command::new(&bin)
        .args(args)
        .env_remove("NXR_AUTH")
        .env_remove("NXR_USERNAME")
        .env_remove("NXR_PASSWORD")
        .env_remove("NXR_CONFIG")
        .output()
        .map_err(|e| format!("{} did not run: {e}", bin.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} {} exited with {:?}: {}",
            bin.display(),
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// A server that dribbles every response body: the slow-path variant of `down`.
///
/// The delay is what makes the pool matter: with a fast server the transfer is
/// dominated by local work, with a dribbling one an idle worker shows up as
/// lost throughput.
#[must_use]
pub fn slow_server() -> mock_nexus::Scenario {
    /// Milliseconds a chunk of a body waits before the next one: a real proxy number.
    const CHUNK_DELAY_MS: u64 = 5;
    /// Bytes per chunk: small enough that the delay is felt, large enough to stream.
    const CHUNK_SIZE: usize = 1024;
    mock_nexus::Scenario::Slow {
        chunk_delay_ms: CHUNK_DELAY_MS,
        chunk_size: CHUNK_SIZE,
    }
}

/// The shared async runtime: multi-thread, all drivers enabled.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}
