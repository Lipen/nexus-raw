//! Shared parts of the bench workspace: the deterministic tree and the in-process mock.
//!
//! The benches and the `gen-tree` binary both go through this module, so a tree
//! a human generates by hand and a tree a bench builds are the same bytes.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
    /// The default the recipes run: 200 files, 4 levels of 4, 2 KiB each.
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

    /// The heavy walk for `scan`: 2000 files over the same shape.
    #[must_use]
    pub fn heavy() -> Self {
        Self {
            files: 2000,
            ..Self::standard()
        }
    }

    /// A stable tag for this shape, used as the tree directory name.
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

/// One SplitMix64 step: the whole generator is three multiplies, no dependencies.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The directory the benches build their trees in: under the bench target, never committed.
#[must_use]
pub fn tree_root(spec: TreeSpec) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench-trees")
        .join(spec.tag())
}

/// The directory the `down` bench downloads into: next to the trees, wiped by its setup.
#[must_use]
pub fn down_target(spec: TreeSpec) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("bench-trees")
        .join(format!("down-{}", spec.tag()))
}

/// (Re)build the tree at `root`: removed first, then written file by file.
/// A tree from an earlier run never leaks into this one: no stale markers, no stale bytes.
pub fn write_tree_at(root: &Path, spec: TreeSpec) -> std::io::Result<()> {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root)?;
    for i in 0..spec.files {
        let path = root.join(spec.file_name(i));
        fs::create_dir_all(path.parent().expect("file names carry a parent"))?;
        fs::write(path, spec.file_bytes(i))?;
    }
    Ok(())
}

/// (Re)build the canonical tree for `spec` under the bench target.
pub fn write_tree(spec: TreeSpec) -> std::io::Result<PathBuf> {
    let root = tree_root(spec);
    write_tree_at(&root, spec)?;
    Ok(root)
}

/// The bench configuration: localhost transport, no credentials, a modest fan-out.
#[must_use]
pub fn config(base: String) -> Config {
    Config {
        base,
        tls_insecure: false,
        workers: 8,
        retry_attempts: 2,
        connect_timeout: Duration::from_secs(5),
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
    pub fn start() -> Self {
        let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic)
            .expect("the mock binds a free port");
        let dir = format!("{}bench/", server.base_url());
        let (tx, rx) = mpsc::unbounded_channel();
        let nxr = Nxr::new(config(dir.clone()), tx).expect("the mock URL is a valid base");
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

    /// A second facade over the same server: a fresh connection pool, same directory.
    /// The event channel is dropped with the call: every core send site ignores a closed receiver.
    /// The benches use it to retry an iteration that lost a request to localhost noise.
    pub fn facade(&self) -> Nxr {
        let (tx, _rx) = mpsc::unbounded_channel();
        Nxr::new(config(self.dir.clone()), tx).expect("the mock URL is a valid base")
    }
}

/// The shared async runtime: multi-thread, all drivers enabled.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}
