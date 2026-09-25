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
    /// sibling present but unparseable — a foreign object (§5.1: always refused).
    Broken(String),
}

pub fn bytes_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(name.as_str())
}

pub fn sibling_path(dir: &Path, name: &ArtifactName) -> PathBuf {
    dir.join(name.sibling())
}

/// Local state classification. Bytes decide: a sibling without bytes is Absent.
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
