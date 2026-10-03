//! Symmetric diff: classification into a deterministic `Action` list (§5.2).

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Verdict;
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::state::{bytes_path, local_status, LocalStatus, RemoteStatus};
use crate::model::{digest, sibling};

/// Which side the plan is for.
/// The classification is symmetric: the mode only decides what a name with a single completed copy means (fetch it, or refuse to shadow it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Up,
    Down,
}

/// One name's action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Both sides agree (or the local copy is already the truth for down).
    Skip { name: ArtifactName, digest: Digest },
    /// Send the local bytes.
    /// `digest == Some` also writes the canonical marker.
    /// `None` means the caller opted out of markers (`--no-sha`).
    Upload {
        name: ArtifactName,
        size: u64,
        digest: Option<Digest>,
    },
    /// Fetch the remote bytes.
    /// The remote sibling digest when present.
    Download {
        name: ArtifactName,
        size: Option<u64>,
        digest: Option<Digest>,
    },
}

/// Local states of the given names, in that order.
pub async fn local_statuses(
    dir: &Path,
    names: Vec<ArtifactName>,
) -> Vec<(ArtifactName, LocalStatus)> {
    let dir = dir.to_owned();
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

/// Symmetric diff.
/// A refusal is a [`Verdict`]: the first violation in name order.
/// `Missing` is collected across all names and fires only when no other refusal exists.
///
/// The §5.2 gap "local Markerless, remote Absent" counts as Missing for down: no completed copy exists anywhere.
pub fn classify(
    dir: &Path,
    mode: Mode,
    markers: bool,
    locals: Vec<(ArtifactName, LocalStatus)>,
    remotes: BTreeMap<ArtifactName, RemoteStatus>,
) -> Result<Vec<Action>, Verdict> {
    // `markers` decides whether Up actions carry a marker digest.
    // Knowing the digest and writing the marker are separate concerns: `--no-sha` still hashes for the divergence checks, it just opts out of writing.
    let up_digest = |d: Digest| markers.then_some(d);
    let mut actions = Vec::with_capacity(locals.len());
    let mut missing: Vec<String> = Vec::new();
    for (name, local) in locals {
        let remote = remotes.get(&name).cloned().unwrap_or(RemoteStatus::Absent);
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
            (LocalStatus::Complete(d), RemoteStatus::Absent) => match mode {
                Mode::Up => {
                    let size = std::fs::metadata(bytes_path(dir, &name)).map_or(0, |m| m.len());
                    actions.push(Action::Upload {
                        name,
                        size,
                        digest: up_digest(d.clone()),
                    });
                }
                Mode::Down => actions.push(Action::Skip {
                    name,
                    digest: d.clone(),
                }),
            },
            (LocalStatus::Complete(d), RemoteStatus::Markerless { .. }) => match mode {
                // The remote copy was never finished: send the finished one.
                Mode::Up => {
                    let size = std::fs::metadata(bytes_path(dir, &name)).map_or(0, |m| m.len());
                    actions.push(Action::Upload {
                        name,
                        size,
                        digest: up_digest(d.clone()),
                    });
                }
                Mode::Down => actions.push(Action::Skip {
                    name,
                    digest: d.clone(),
                }),
            },
            (LocalStatus::Markerless, RemoteStatus::Complete { digest, size }) => match mode {
                Mode::Up => {
                    let actual = local_digest(dir, &name)?;
                    if &actual == digest {
                        actions.push(Action::Skip {
                            name,
                            digest: actual,
                        });
                    } else {
                        return Err(Verdict::Mismatch {
                            name: name.to_string(),
                            detail: format!(
                                "local bytes diverge from the remote marker: local {actual}, remote {digest}"
                            ),
                        });
                    }
                }
                Mode::Down => {
                    actions.push(Action::Download {
                        name,
                        size: *size,
                        digest: Some(digest.clone()),
                    });
                }
            },
            (LocalStatus::Markerless, RemoteStatus::Markerless { .. }) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: "unverifiable on both sides: no marker anywhere; fetch with down or delete one copy".into(),
                });
            }
            (LocalStatus::Markerless, RemoteStatus::Absent) => match mode {
                Mode::Up => {
                    let size = std::fs::metadata(bytes_path(dir, &name)).map_or(0, |m| m.len());
                    let d = local_digest(dir, &name)?;
                    actions.push(Action::Upload {
                        name,
                        size,
                        digest: up_digest(d),
                    });
                }
                Mode::Down => missing.push(name.to_string()),
            },
            (LocalStatus::Broken(detail), _) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("local object is broken and must not be overwritten: {detail}"),
                });
            }
            (LocalStatus::Absent, RemoteStatus::Absent) => missing.push(name.to_string()),
            (LocalStatus::Absent, RemoteStatus::Complete { digest, size }) => match mode {
                Mode::Up => {
                    return Err(Verdict::Mismatch {
                        name: name.to_string(),
                        detail: "exists remotely but not locally; up never deletes, fetch with down or remove it remotely".into(),
                    });
                }
                Mode::Down => {
                    actions.push(Action::Download {
                        name,
                        size: *size,
                        digest: Some(digest.clone()),
                    });
                }
            },
            // Under-uploaded on the server: fetch the bytes, compute the marker.
            (LocalStatus::Absent, RemoteStatus::Markerless { size }) => match mode {
                Mode::Up => {
                    return Err(Verdict::Mismatch {
                        name: name.to_string(),
                        detail: "exists remotely but not locally; up never deletes, fetch with down or remove it remotely".into(),
                    });
                }
                Mode::Down => {
                    actions.push(Action::Download {
                        name,
                        size: *size,
                        digest: None,
                    });
                }
            },
            // A foreign or unparseable sibling refuses the name: the remote object is never overwritten.
            (_, RemoteStatus::Broken(detail)) => {
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

/// The digest of a local file that has no marker yet.
fn local_digest(dir: &Path, name: &ArtifactName) -> Result<Digest, Verdict> {
    digest::sha256_file(&bytes_path(dir, name)).map_err(|e| Verdict::Mismatch {
        name: name.to_string(),
        detail: format!("bytes unreadable: {e}"),
    })
}

/// The canonical marker line for a name and digest: `<hex>  <name>\n`.
#[must_use]
pub fn marker_line(name: &ArtifactName, digest: &Digest) -> String {
    sibling::format_line(name.as_str(), digest)
}
