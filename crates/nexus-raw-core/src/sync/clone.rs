//! The clone executor: a verified local copy of a whole repository, with an
//! optional local fact cache.
//!
//! Clone is `down` over a repository-wide enumeration, plus two things `down`
//! does not do: a local fact cache that skips re-hashing files whose size and
//! mtime still match the last verified copy, and a seal for a local object
//! whose bytes already match its remote marker but whose local marker is gone.
//!
//! The cache is an accelerator, never authority.
//! A missing, corrupt or foreign cache falls back to hashing. `--verify`
//! ignores it entirely. `verify`, `audit` and `doctor` never read it.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Verdict, DATA_EXIT};
use crate::events::{Dir, Failure, Summary};
use crate::model::digest::{sha256_file, Digest};
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path, RemoteStatus};
use crate::sync::down::{download_one, ensure_symlink_free, Outcome};
use crate::transport::aimd::WorkerLimit;
use crate::transport::client::NexusClient;

/// The default cache name inside the clone destination.
pub const DEFAULT_STATE_FILE: &str = ".nxr-state.json";
/// The cache format version. A different version is ignored, never migrated.
const STATE_VERSION: u32 = 1;
/// How many successful names may land between cache flushes.
const SAVE_EVERY: usize = 64;

/// Where a clone keeps its local fact cache.
#[derive(Debug, Clone, Copy, Default)]
pub enum StateMode<'a> {
    /// `<DIR>/.nxr-state.json`.
    #[default]
    Default,
    /// An explicit file.
    At(&'a Path),
    /// No cache at all.
    Off,
}

impl StateMode<'_> {
    /// The cache path this mode resolves to for `dir`.
    #[must_use]
    pub fn path(self, dir: &Path) -> Option<PathBuf> {
        match self {
            StateMode::Default => Some(dir.join(DEFAULT_STATE_FILE)),
            StateMode::At(path) => Some(path.to_owned()),
            StateMode::Off => None,
        }
    }
}

/// One artifact of the clone plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneAction {
    /// Fetch the bytes. The marker is written from the verified digest.
    Download {
        name: ArtifactName,
        size: Option<u64>,
        /// The remote marker digest, when one exists.
        digest: Option<Digest>,
    },
    /// The local copy is complete and matches the remote marker.
    Skip { name: ArtifactName, digest: Digest },
    /// The local bytes match the remote marker, but the local marker is gone:
    /// write it from the verified digest without fetching the object again.
    Seal {
        name: ArtifactName,
        digest: Digest,
        size: Option<u64>,
    },
}

/// A local classification that also carries the digest of a markerless object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LocalFact {
    Complete(Digest),
    Markerless { digest: Digest, size: u64 },
    Broken(String),
    Absent,
}

/// The local fact cache: one entry per verified object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CloneState {
    version: u32,
    base: String,
    entries: BTreeMap<String, StateEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StateEntry {
    digest: String,
    size: u64,
    mtime_ns: u64,
}

impl CloneState {
    /// An empty cache for this base.
    pub(crate) fn empty(base: &str) -> Self {
        Self {
            version: STATE_VERSION,
            base: base.to_owned(),
            entries: BTreeMap::new(),
        }
    }

    /// Load a cache, falling back to empty on any mismatch.
    ///
    /// A missing file, a corrupt file, another format version or another base
    /// all mean "hash everything": the cache never carries authority.
    pub(crate) fn load(path: &Path, base: &str) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::empty(base);
        };
        let Ok(state) = serde_json::from_slice::<Self>(&bytes) else {
            return Self::empty(base);
        };
        if state.version != STATE_VERSION || state.base != base {
            return Self::empty(base);
        }
        state
    }

    /// Write the cache.
    ///
    /// Losing it costs hashes, never correctness, so a plain write is enough:
    /// a torn file is ignored by the next load.
    fn save(&self, path: &Path) -> Result<(), Error> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| Error::io(path, std::io::Error::other(e)))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
        }
        // The cache path lives in a user-chosen directory: a planted symlink
        // must never redirect the write. O_NOFOLLOW refuses it on unix.
        let mut f = crate::transport::client::write_options_blocking(false)
            .open(path)
            .map_err(|e| Error::io(path, e))?;
        f.write_all(&bytes).map_err(|e| Error::io(path, e))?;
        f.flush().map_err(|e| Error::io(path, e))
    }

    fn digest_of(&self, name: &ArtifactName, size: u64, mtime_ns: u64) -> Option<Digest> {
        if mtime_ns == 0 {
            return None;
        }
        let entry = self.entries.get(name.as_str())?;
        if entry.size != size || entry.mtime_ns != mtime_ns {
            return None;
        }
        Digest::from_hex(&entry.digest).ok()
    }
}

/// Classify every name into a clone action.
///
/// The refusal rules are `down`'s: a broken local object or a foreign remote
/// marker is never overwritten, and a name the index lists but the storage
/// does not hold is collected as missing.
///
/// # Errors
///
/// Returns the first [`Verdict`] refusal in name order, or the collected
/// [`Verdict::Missing`] when an enumerated name is absent at the storage.
pub(crate) fn classify(
    locals: Vec<(ArtifactName, LocalFact)>,
    remotes: BTreeMap<ArtifactName, RemoteStatus>,
) -> Result<Vec<CloneAction>, Verdict> {
    let mut actions = Vec::with_capacity(locals.len());
    let mut missing = Vec::new();
    for (name, local) in locals {
        let remote = remotes.get(&name).cloned().unwrap_or(RemoteStatus::Absent);
        match (&local, &remote) {
            (LocalFact::Broken(detail), _) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: crate::model::state::broken_detail(detail),
                });
            }
            (_, RemoteStatus::Broken(detail)) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("remote sibling is foreign/unparseable: {detail}"),
                });
            }
            (_, RemoteStatus::Absent) => missing.push(name.to_string()),
            (LocalFact::Absent, RemoteStatus::Complete { digest, size }) => {
                actions.push(CloneAction::Download {
                    name,
                    size: *size,
                    digest: Some(digest.clone()),
                });
            }
            (LocalFact::Absent, RemoteStatus::Markerless { size }) => {
                actions.push(CloneAction::Download {
                    name,
                    size: *size,
                    digest: None,
                });
            }
            (LocalFact::Complete(local), RemoteStatus::Complete { digest, size }) => {
                if local == digest {
                    actions.push(CloneAction::Skip {
                        name,
                        digest: local.clone(),
                    });
                } else {
                    actions.push(CloneAction::Download {
                        name,
                        size: *size,
                        digest: Some(digest.clone()),
                    });
                }
            }
            // The remote has no marker: the bytes are unverifiable, so the
            // clone fetches them and computes the marker from what it received.
            (LocalFact::Complete(_), RemoteStatus::Markerless { size }) => {
                actions.push(CloneAction::Download {
                    name,
                    size: *size,
                    digest: None,
                });
            }
            (
                LocalFact::Markerless { digest: local, .. },
                RemoteStatus::Complete { digest, size },
            ) => {
                if local == digest {
                    actions.push(CloneAction::Seal {
                        name,
                        digest: local.clone(),
                        size: *size,
                    });
                } else {
                    actions.push(CloneAction::Download {
                        name,
                        size: *size,
                        digest: Some(digest.clone()),
                    });
                }
            }
            (LocalFact::Markerless { .. }, RemoteStatus::Markerless { size }) => {
                actions.push(CloneAction::Download {
                    name,
                    size: *size,
                    digest: None,
                });
            }
        }
    }
    if !missing.is_empty() {
        return Err(Verdict::Missing { names: missing });
    }
    Ok(actions)
}

/// Classify the local tree, hashing only what the cache cannot vouch for.
///
/// # Panics
///
/// The classification runs in `spawn_blocking`. A panic there is an internal
/// invariant break, and the same expectation the other blocking scans use.
pub(crate) async fn local_facts(
    dir: &Path,
    names: Vec<ArtifactName>,
    state: CloneState,
    verify: bool,
) -> Vec<(ArtifactName, LocalFact)> {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || {
        names
            .into_iter()
            .map(|name| {
                let fact = local_fact(&dir, &name, &state, verify);
                (name, fact)
            })
            .collect()
    })
    .await
    .expect("local classification cannot panic")
}

fn local_fact(dir: &Path, name: &ArtifactName, state: &CloneState, verify: bool) -> LocalFact {
    let bytes = bytes_path(dir, name);
    let meta = match std::fs::symlink_metadata(&bytes) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return LocalFact::Broken("not a regular file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LocalFact::Absent,
        Err(e) => return LocalFact::Broken(format!("bytes unreadable: {e}")),
    };
    let size = meta.len();
    let mtime_ns = mtime_ns(&meta);
    let sib = sibling_path(dir, name);
    let marker = match std::fs::read_to_string(&sib) {
        Ok(raw) => match sibling::parse_line(&raw) {
            Ok(parsed) => Some(parsed),
            Err(e) => return LocalFact::Broken(format!("sibling unparseable: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return LocalFact::Broken(format!("sibling unreadable: {e}")),
    };
    match marker {
        Some(parsed) => {
            if !verify {
                if let Some(cached) = state.digest_of(name, size, mtime_ns) {
                    if cached == parsed.digest {
                        return LocalFact::Complete(cached);
                    }
                }
            }
            match sha256_file(&bytes) {
                Ok(actual) if actual == parsed.digest => LocalFact::Complete(actual),
                Ok(actual) => LocalFact::Broken(format!(
                    "digest mismatch: sibling {}, actual {}",
                    parsed.digest, actual
                )),
                Err(e) => LocalFact::Broken(format!("bytes unreadable: {e}")),
            }
        }
        // No marker: hash once and let the classifier decide between sealing
        // (the remote marker matches) and downloading.
        None => match sha256_file(&bytes) {
            Ok(digest) => LocalFact::Markerless { digest, size },
            Err(e) => LocalFact::Broken(format!("bytes unreadable: {e}")),
        },
    }
}

/// Execute the clone plan.
///
/// Downloads ride the same `down` machinery (part resume, digest check,
/// marker, rename). Seals write only the missing local marker. Skips touch
/// nothing. The summary goes out before any error returns.
///
/// # Errors
///
/// Returns the first transport/auth/HTTP failure of the run and
/// [`Error::Mismatch`] on a digest divergence.
pub(crate) async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    base: String,
    actions: Vec<CloneAction>,
    mut state: CloneState,
    state_path: Option<PathBuf>,
    workers: Arc<WorkerLimit>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut summary = Summary::default();
    let mut failed: Vec<(String, Failure)> = Vec::new();
    let mut mismatched: Vec<(String, Failure)> = Vec::new();
    let mut first_error: Option<Error> = None;
    let mut since_save = 0usize;

    for action in actions {
        match action {
            CloneAction::Skip { name, digest } => {
                summary.skipped += 1;
                record(&mut state, &dir, &name, &digest);
                since_save += 1;
                client
                    .progress()
                    .done(name.as_str(), Dir::Down, true, 0, None)
                    .await;
            }
            CloneAction::Seal { name, digest, size } => match seal(&dir, &name, &digest).await {
                Ok(()) => {
                    summary.sealed += 1;
                    record(&mut state, &dir, &name, &digest);
                    since_save += 1;
                    client
                        .progress()
                        .done(name.as_str(), Dir::Down, true, size.unwrap_or(0), size)
                        .await;
                }
                Err(e) => {
                    summary.failed.push(name.to_string());
                    summary.failures.push(Failure {
                        name: name.to_string(),
                        code: e.exit_code(),
                    });
                    log::error!("clone seal failed: {e}");
                    first_error.get_or_insert(e);
                }
            },
            CloneAction::Download { name, size, digest } => {
                let permit = workers.acquire().await.map_err(Error::misuse)?;
                let client = client.clone();
                let dir = dir.clone();
                let base = base.clone();
                let workers = workers.clone();
                set.spawn(async move {
                    let result =
                        download_one(&client, &dir, &base, &name, size, digest, false, true).await;
                    drop(permit);
                    match result {
                        Ok(()) => {
                            workers.release_success();
                            Ok(name)
                        }
                        Err(e) => {
                            workers.release_failure();
                            Err(e)
                        }
                    }
                });
            }
        }
        if since_save >= SAVE_EVERY {
            flush(&state, state_path.as_deref())?;
            since_save = 0;
        }
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(name)) => {
                summary.downloaded += 1;
                if let Some(digest) = marker_digest(&dir, &name) {
                    record(&mut state, &dir, &name, &digest);
                }
                since_save += 1;
            }
            Ok(Err((name, Outcome::Failed(e)))) => {
                failed.push((
                    name.to_string(),
                    Failure {
                        name: name.to_string(),
                        code: e.exit_code(),
                    },
                ));
                log::error!("clone failed: {e}");
                first_error.get_or_insert(e);
            }
            Ok(Err((name, Outcome::Mismatch(detail)))) => {
                mismatched.push((
                    format!("{name}: {detail}"),
                    Failure {
                        name: name.to_string(),
                        code: DATA_EXIT,
                    },
                ));
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
        if since_save >= SAVE_EVERY {
            flush(&state, state_path.as_deref())?;
            since_save = 0;
        }
    }
    summary.failed = failed
        .iter()
        .map(|(legacy, _)| legacy.clone())
        .chain(mismatched.iter().map(|(legacy, _)| legacy.clone()))
        .collect();
    summary.failures = failed
        .iter()
        .map(|(_, failure)| failure.clone())
        .chain(mismatched.iter().map(|(_, failure)| failure.clone()))
        .collect();
    client.progress().summary(&summary);
    flush(&state, state_path.as_deref())?;
    if !mismatched.is_empty() {
        return Err(Error::Mismatch {
            name: mismatched
                .iter()
                .map(|(legacy, _)| legacy.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            detail: "remote content diverges from its sha-sibling".to_owned(),
        });
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(summary)
}

fn flush(state: &CloneState, path: Option<&Path>) -> Result<(), Error> {
    match path {
        Some(path) => state.save(path),
        None => Ok(()),
    }
}

/// Write the missing local marker from a digest the bytes have just matched.
async fn seal(dir: &Path, name: &ArtifactName, digest: &Digest) -> Result<(), Error> {
    let bytes = bytes_path(dir, name);
    if let Some(parent) = bytes.parent() {
        ensure_symlink_free(parent, dir)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::io(parent, e))?;
    }
    let sib = sibling_path(dir, name);
    sibling::write_marker(&sib, name.as_str(), digest).await
}

fn marker_digest(dir: &Path, name: &ArtifactName) -> Option<Digest> {
    let raw = std::fs::read_to_string(sibling_path(dir, name)).ok()?;
    sibling::parse_line(&raw).ok().map(|parsed| parsed.digest)
}

/// Refresh the cache entry of one verified name.
fn record(state: &mut CloneState, dir: &Path, name: &ArtifactName, digest: &Digest) {
    let Ok(meta) = std::fs::symlink_metadata(bytes_path(dir, name)) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    state.entries.insert(
        name.as_str().to_owned(),
        StateEntry {
            digest: digest.as_str().to_owned(),
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
        },
    );
}

fn mtime_ns(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(raw: &str) -> ArtifactName {
        ArtifactName::parse(raw).unwrap()
    }

    #[test]
    fn classify_covers_download_skip_seal_and_divergence() {
        let d = Digest::of_bytes(b"x");
        let e = Digest::of_bytes(b"y");
        let locals = vec![
            (name("absent.bin"), LocalFact::Absent),
            (name("same.bin"), LocalFact::Complete(d.clone())),
            (name("stale.bin"), LocalFact::Complete(d.clone())),
            (
                name("seal.bin"),
                LocalFact::Markerless {
                    digest: d.clone(),
                    size: 1,
                },
            ),
            (
                name("seal-diverged.bin"),
                LocalFact::Markerless {
                    digest: e.clone(),
                    size: 1,
                },
            ),
        ];
        let remotes = BTreeMap::from([
            (
                name("absent.bin"),
                RemoteStatus::Complete {
                    digest: d.clone(),
                    size: Some(1),
                },
            ),
            (
                name("same.bin"),
                RemoteStatus::Complete {
                    digest: d.clone(),
                    size: Some(1),
                },
            ),
            (
                name("stale.bin"),
                RemoteStatus::Complete {
                    digest: e.clone(),
                    size: Some(1),
                },
            ),
            (
                name("seal.bin"),
                RemoteStatus::Complete {
                    digest: d.clone(),
                    size: Some(1),
                },
            ),
            (
                name("seal-diverged.bin"),
                RemoteStatus::Complete {
                    digest: d.clone(),
                    size: Some(1),
                },
            ),
        ]);
        let actions = classify(locals, remotes).unwrap();
        assert!(matches!(actions[0], CloneAction::Download { .. }));
        assert!(matches!(actions[1], CloneAction::Skip { .. }));
        assert!(matches!(actions[2], CloneAction::Download { .. }));
        assert!(matches!(actions[3], CloneAction::Seal { .. }));
        assert!(matches!(actions[4], CloneAction::Download { .. }));
    }

    #[test]
    fn classify_refuses_a_broken_local_object() {
        let remotes = BTreeMap::from([(
            name("a.bin"),
            RemoteStatus::Complete {
                digest: Digest::of_bytes(b"x"),
                size: Some(1),
            },
        )]);
        let err = classify(
            vec![(name("a.bin"), LocalFact::Broken("boom".into()))],
            remotes,
        )
        .unwrap_err();
        assert!(matches!(err, Verdict::Mismatch { .. }));
    }

    #[test]
    fn classify_collects_storage_absent_names() {
        let remotes = BTreeMap::from([(name("ghost.bin"), RemoteStatus::Absent)]);
        let err = classify(vec![(name("ghost.bin"), LocalFact::Absent)], remotes).unwrap_err();
        assert!(matches!(err, Verdict::Missing { names } if names == ["ghost.bin".to_owned()]));
    }

    #[test]
    fn state_ignores_another_base() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut state = CloneState::empty("http://a/repository/r/");
        state.entries.insert(
            "x.bin".to_owned(),
            StateEntry {
                digest: "00".repeat(32),
                size: 1,
                mtime_ns: 2,
            },
        );
        state.save(&path).unwrap();
        assert_eq!(
            CloneState::load(&path, "http://a/repository/r/")
                .entries
                .len(),
            1
        );
        assert!(CloneState::load(&path, "http://b/repository/r/")
            .entries
            .is_empty());
    }

    #[test]
    fn state_digest_requires_size_and_mtime() {
        let mut state = CloneState::empty("b");
        let digest = Digest::of_bytes(b"x");
        state.entries.insert(
            "x.bin".to_owned(),
            StateEntry {
                digest: digest.as_str().to_owned(),
                size: 1,
                mtime_ns: 2,
            },
        );
        assert_eq!(state.digest_of(&name("x.bin"), 1, 2), Some(digest));
        assert_eq!(state.digest_of(&name("x.bin"), 2, 2), None);
        assert_eq!(state.digest_of(&name("x.bin"), 1, 3), None);
        assert_eq!(state.digest_of(&name("x.bin"), 1, 0), None);
    }
}
