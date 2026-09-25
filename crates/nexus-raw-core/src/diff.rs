//! Symmetric diff: the §5.2 classification into a deterministic `Action` list.

use std::collections::BTreeMap;
use std::path::Path;

use crate::model::claim::Claim;
use crate::model::digest::Digest;
use crate::error::Verdict;
use crate::model::name::ArtifactName;
use crate::model::state::{bytes_path, local_status, LocalStatus, RemoteStatus};

/// One name's action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Skip {
        name: ArtifactName,
        digest: Digest,
    },
    /// Send the local bytes where they are missing (up).
    Upload {
        name: ArtifactName,
        size: u64,
    },
    /// Fetch the remote bytes (down); the remote sibling digest when present.
    Download {
        name: ArtifactName,
        size: Option<u64>,
        digest: Option<Digest>,
    },
}

/// Local states of all claim names, in claim order.
pub async fn local_statuses(dir: &Path, claim: &Claim) -> Vec<(ArtifactName, LocalStatus)> {
    let dir = dir.to_owned();
    let names = claim.artifacts.clone();
    tokio::task::spawn_blocking(move || {
        names
            .into_iter()
            .map(|n| {
                let st = local_status(&dir, &n);
                (n, st)
            })
            .collect()
    })
    .await
    .expect("local classification cannot panic")
}

/// Symmetric diff. A refusal is a [`Verdict`]: the first violation in claim order;
/// `Missing` is collected across all names and fires only when no other refusal exists.
///
/// The §5.2 gap "local Markerless, remote Absent" is treated as Missing:
/// no completed copy exists anywhere.
pub fn classify(
    dir: &Path,
    claim: &Claim,
    locals: Vec<(ArtifactName, LocalStatus)>,
    remotes: BTreeMap<ArtifactName, RemoteStatus>,
) -> Result<Vec<Action>, Verdict> {
    let mut actions = Vec::with_capacity(claim.artifacts.len());
    let mut missing: Vec<String> = Vec::new();
    for (name, local) in locals {
        let remote = remotes
            .get(&name)
            .cloned()
            .unwrap_or(RemoteStatus::Absent);
        match (&local, &remote) {
            (LocalStatus::Complete(ld), RemoteStatus::Complete { digest: rd, .. }) => {
                if ld == rd {
                    actions.push(Action::Skip {
                        name,
                        digest: ld.clone(),
                    });
                } else {
                    return Err(Verdict::Mismatch {
                        name: name.to_string(),
                        detail: format!(
                            "complete on both sides with different digests: local {ld}, remote {rd}"
                        ),
                    });
                }
            }
            (LocalStatus::Complete(_), RemoteStatus::Absent | RemoteStatus::Markerless { .. }) => {
                let size = std::fs::metadata(bytes_path(dir, &name))
                    .map(|m| m.len())
                    .unwrap_or(0);
                actions.push(Action::Upload { name, size });
            }
            (
                LocalStatus::Markerless,
                RemoteStatus::Complete { digest, size },
            ) => {
                // The remote sibling is the truth: re-verify locally (§5.2).
                actions.push(Action::Download {
                    name,
                    size: *size,
                    digest: Some(digest.clone()),
                });
            }
            (LocalStatus::Markerless, RemoteStatus::Markerless { .. }) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: "markerless on both sides: nothing to verify against".into(),
                });
            }
            (LocalStatus::Markerless, RemoteStatus::Absent) => missing.push(name.to_string()),
            (LocalStatus::Broken(detail), _) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("local build is broken and must not be overwritten: {detail}"),
                });
            }
            (LocalStatus::Absent, RemoteStatus::Absent) => missing.push(name.to_string()),
            (
                LocalStatus::Absent,
                RemoteStatus::Complete { digest, size },
            ) => {
                actions.push(Action::Download {
                    name,
                    size: *size,
                    digest: Some(digest.clone()),
                });
            }
            // Under-uploaded on the server: fetch the bytes, compute the marker (§5.2).
            (LocalStatus::Absent, RemoteStatus::Markerless { size }) => {
                actions.push(Action::Download {
                    name,
                    size: *size,
                    digest: None,
                });
            }
            // A sibling without bytes means the remote object is not complete.
            (_, RemoteStatus::Broken(detail)) => {
                // §5.1: Broken is always refused; a foreign object is never overwritten.
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("remote sibling is foreign/unparseable: {detail}"),
                });
            }
        }
    }
    if !missing.is_empty() {
        return Err(Verdict::Missing { names: missing });
    }
    Ok(actions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> ArtifactName {
        ArtifactName::parse(s).unwrap()
    }

    fn claim_of(names: &[&str]) -> Claim {
        Claim {
            claim_version: 1,
            version: "1.0.0".into(),
            artifacts: names.iter().map(|s| name(s)).collect(),
        }
    }

    #[test]
    fn skip_when_complete_and_equal() {
        let claim = claim_of(&["a.zip"]);
        let d = Digest::of_bytes(b"x");
        let locals = vec![(name("a.zip"), LocalStatus::Complete(d.clone()))];
        let remotes = BTreeMap::from([(
            name("a.zip"),
            RemoteStatus::Complete {
                digest: d,
                size: Some(1),
            },
        )]);
        let actions = classify(Path::new("/nonexistent"), &claim, locals, remotes).unwrap();
        assert!(matches!(&actions[..], [Action::Skip { .. }]));
    }

    #[test]
    fn mismatch_on_digest_difference() {
        let claim = claim_of(&["a.zip"]);
        let locals = vec![(name("a.zip"), LocalStatus::Complete(Digest::of_bytes(b"x")))];
        let remotes = BTreeMap::from([(
            name("a.zip"),
            RemoteStatus::Complete {
                digest: Digest::of_bytes(b"y"),
                size: Some(1),
            },
        )]);
        match classify(Path::new("/nonexistent"), &claim, locals, remotes) {
            Err(Verdict::Mismatch { name, .. }) => assert_eq!(name, "a.zip"),
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn broken_local_never_overwritten() {
        let claim = claim_of(&["a.zip"]);
        let locals = vec![(
            name("a.zip"),
            LocalStatus::Broken("digest mismatch".into()),
        )];
        let remotes = BTreeMap::from([(
            name("a.zip"),
            RemoteStatus::Complete {
                digest: Digest::zero(),
                size: Some(1),
            },
        )]);
        assert!(matches!(
            classify(Path::new("/nonexistent"), &claim, locals, remotes),
            Err(Verdict::Mismatch { .. })
        ));
    }

    #[test]
    fn missing_when_nowhere() {
        let claim = claim_of(&["a.zip", "b.zip"]);
        let locals = vec![
            (name("a.zip"), LocalStatus::Absent),
            (name("b.zip"), LocalStatus::Absent),
        ];
        let remotes = BTreeMap::from([
            (name("a.zip"), RemoteStatus::Absent),
            (name("b.zip"), RemoteStatus::Absent),
        ]);
        match classify(Path::new("/nonexistent"), &claim, locals, remotes) {
            Err(Verdict::Missing { names }) => assert_eq!(names.len(), 2),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn markerless_remote_download_without_digest() {
        let claim = claim_of(&["a.zip"]);
        let locals = vec![(name("a.zip"), LocalStatus::Absent)];
        let remotes = BTreeMap::from([(
            name("a.zip"),
            RemoteStatus::Markerless { size: Some(10) },
        )]);
        let actions = classify(Path::new("/nonexistent"), &claim, locals, remotes).unwrap();
        assert!(matches!(
            &actions[..],
            [Action::Download { digest: None, .. }]
        ));
    }
}
