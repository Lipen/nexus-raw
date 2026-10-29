//! Local and remote per-name state (protocol §5.1).

use std::path::{Path, PathBuf};

use super::digest::{sha256_file, Digest};
use super::name::ArtifactName;
use super::sibling;

/// Local state of a name inside the version directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalStatus {
    /// bytes + valid sibling, digest matches.
    Complete(Digest),
    /// bytes present, sibling absent.
    Markerless,
    /// sibling present, digest mismatches or does not parse.
    Broken(String),
    /// no bytes.
    Absent,
}

/// Remote state of a name (HEAD of bytes + GET of sibling).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStatus {
    Complete {
        digest: Digest,
        size: Option<u64>,
    },
    Markerless {
        size: Option<u64>,
    },
    Absent,
    /// sibling present but unparseable, a foreign object (§5.1: always refused).
    Broken(String),
}

#[must_use]
pub fn bytes_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(name.as_str())
}

#[must_use]
pub fn sibling_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(name.sibling())
}

/// Local state classification.
/// Bytes decide: a sibling without bytes is Absent.
#[must_use]
pub fn local_status(dir: &Path, name: &ArtifactName) -> LocalStatus {
    let bytes = bytes_path(dir, name);
    if !bytes.is_file() {
        return LocalStatus::Absent;
    }
    let sib = sibling_path(dir, name);
    if !sib.is_file() {
        return LocalStatus::Markerless;
    }
    let raw = match std::fs::read_to_string(&sib) {
        Ok(r) => r,
        Err(e) => return LocalStatus::Broken(format!("sibling unreadable: {e}")),
    };
    let parsed = match sibling::parse_line(&raw) {
        Ok(p) => p,
        Err(e) => return LocalStatus::Broken(format!("sibling unparseable: {e}")),
    };
    match sha256_file(&bytes) {
        Ok(actual) if actual == parsed.digest => LocalStatus::Complete(actual),
        Ok(actual) => LocalStatus::Broken(format!(
            "digest mismatch: sibling {}, actual {}",
            parsed.digest.as_str(),
            actual.as_str()
        )),
        Err(e) => LocalStatus::Broken(format!("bytes unreadable: {e}")),
    }
}

/// The detail of a local bytes/marker disagreement, with the recovery a user
/// can actually take: both halves are local.
///
/// A marker that no longer matches its bytes is common after a rebuild that
/// reused a name. `nxr` never regenerates such a marker by itself, because that
/// would bless an edit a consumer may already hold: the fix is to delete the
/// stale sibling (the next scan regenerates it) or to restore the bytes.
pub(crate) fn broken_detail(detail: &str) -> String {
    format!(
        "local object is broken and must not be overwritten: {detail}. \
         Delete the stale sibling to regenerate it, or restore the bytes, then rerun"
    )
}
