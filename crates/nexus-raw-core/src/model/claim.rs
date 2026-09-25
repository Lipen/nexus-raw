//! claim.json: parse, serialization, sane check (protocol §4.1).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Error;
use super::name::{validate_version, ArtifactName, CLAIM_FILE};

pub const CLAIM_VERSION: u32 = 1;

/// The version's claim.json. Field order is fixed by serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub claim_version: u32,
    pub version: String,
    pub artifacts: Vec<ArtifactName>,
}

#[derive(Serialize, Deserialize)]
struct RawClaim {
    claim_version: u32,
    version: String,
    artifacts: Vec<String>,
}

impl Claim {
    /// Parse + sane check: `claim_version == 1`, version is a valid segment,
    /// every name passes the §3 grammar.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        let raw: RawClaim = serde_json::from_slice(bytes)
            .map_err(|e| Error::misuse(format!("claim.json: {e}")))?;
        if raw.claim_version != CLAIM_VERSION {
            return Err(Error::misuse(format!(
                "claim.json: claim_version {} != {CLAIM_VERSION}",
                raw.claim_version
            )));
        }
        validate_version(&raw.version)?;
        let mut artifacts = Vec::with_capacity(raw.artifacts.len());
        for a in raw.artifacts {
            artifacts.push(ArtifactName::parse(&a)?);
        }
        Ok(Self {
            claim_version: raw.claim_version,
            version: raw.version,
            artifacts,
        })
    }

    pub fn read(path: &Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::from_slice(&bytes)
    }

    /// Deterministic bytes: compact JSON, fixed field order.
    /// The remote comparison is byte-wise (§6.1).
    pub fn to_bytes(&self) -> Vec<u8> {
        let raw = RawClaim {
            claim_version: self.claim_version,
            version: self.version.clone(),
            artifacts: self
                .artifacts
                .iter()
                .map(|a| a.as_str().to_owned())
                .collect(),
        };
        serde_json::to_vec(&raw).expect("claim serialization is infallible")
    }

    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(CLAIM_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"claim_version":1,"version":"1.14.0","artifacts":["pinned.xml","bom/linux-x86_64.json"]}"#;

    #[test]
    fn parse_and_reserialize_deterministic() {
        let claim = Claim::from_slice(OK.as_bytes()).unwrap();
        assert_eq!(claim.version, "1.14.0");
        assert_eq!(claim.artifacts.len(), 2);
        assert_eq!(claim.to_bytes(), OK.as_bytes());
    }

    #[test]
    fn rejects_wrong_claim_version() {
        let bad = OK.replace("\"claim_version\":1", "\"claim_version\":2");
        assert!(Claim::from_slice(bad.as_bytes()).is_err());
    }

    #[test]
    fn rejects_bad_version_segment() {
        let bad = OK.replace("1.14.0", "../etc");
        assert!(Claim::from_slice(bad.as_bytes()).is_err());
    }

    #[test]
    fn rejects_unsafe_artifact_name() {
        let bad = OK.replace("pinned.xml", "../escape.zip");
        match Claim::from_slice(bad.as_bytes()) {
            Err(Error::UnsafeName { name, .. }) => assert_eq!(name, "../escape.zip"),
            other => panic!("expected UnsafeName, got {other:?}"),
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Claim::from_slice(b"not json").is_err());
    }
}
