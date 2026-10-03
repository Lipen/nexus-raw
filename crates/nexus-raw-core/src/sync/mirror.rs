//! The mirror executor: pour enumerated names from a source directory URL to a destination one.
//!
//! Mirror is `up` whose byte source is a GET instead of the local disk.
//! Each copied name rides the existing machinery twice: [`down`] stages it
//! (Range-aware GET into a part file, digest check, rename, marker) and [`up`]
//! pushes it (PUT bytes, then PUT marker).
//! Workers, retries, stall detection and resume are the clients', not ours.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::Digest as _;
use tokio::sync::Semaphore;

use crate::error::{Error, Verdict};
use crate::events::{Dir, Summary};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path, RemoteStatus};
use crate::sync::down::{self, Failure};
use crate::sync::up;
use crate::transport::client::NexusClient;

/// The conventional version-document name.
/// When the enumeration lists it first, the mirror claims it: it transfers alone, before any other name, and a failed claim aborts the run with nothing else sent.
pub const VERSION_DOCUMENT: &str = "version.json";

/// The staging directory of a (source, destination) pair: a stable temp location, so the part files of a killed run are the rerun's resume fuel.
/// The name is a 128-bit truncation of the SHA-256 over both bases: two pairs sharing a staging dir would interleave part writes, and the space stays far beyond birthday reach.
/// The directory is removed on a fully successful run and kept otherwise.
#[must_use]
pub fn staging_dir(src_base: &str, dst_base: &str) -> PathBuf {
    let digest = sha2::Sha256::digest(format!("{src_base}\n{dst_base}").as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    std::env::temp_dir().join(format!("nxr-mirror-{hex}"))
}

/// One name's mirror action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirrorAction {
    /// Both sides hold the same complete object.
    Skip { name: ArtifactName, digest: Digest },
    /// Stage the source bytes, then push bytes and marker to the destination.
    /// `expected` is the source sibling digest: a staged body that diverges from it is refused by the staging machinery, never pushed.
    /// `dst_digest` is set when the destination looked complete but the source had no marker: the staged digest is compared against it before the destination write, so a divergence refuses without touching the destination.
    Copy {
        name: ArtifactName,
        size: Option<u64>,
        expected: Option<Digest>,
        dst_digest: Option<Digest>,
    },
}

/// The mirror diff: source states against destination states, refusal rules as `up`.
/// The plan follows `names` order (the enumeration's), not map order.
/// A refusal is a [`Verdict`]: the first violation in name order.
/// `Missing` is collected across all names and fires only when no other refusal exists.
///
/// # Errors
///
/// Returns the first [`Verdict`] refusal in name order: a broken source or foreign destination sibling, an overwrite refusal, or a collected [`Verdict::Missing`] when a name exists nowhere.
pub fn classify(
    names: Vec<ArtifactName>,
    srcs: BTreeMap<ArtifactName, RemoteStatus>,
    dsts: BTreeMap<ArtifactName, RemoteStatus>,
) -> Result<Vec<MirrorAction>, Verdict> {
    let mut actions = Vec::with_capacity(names.len());
    let mut missing: Vec<String> = Vec::new();
    for name in names {
        let src = srcs.get(&name).cloned().unwrap_or(RemoteStatus::Absent);
        let dst = dsts.get(&name).cloned().unwrap_or(RemoteStatus::Absent);
        match (&src, &dst) {
            // A broken source must not propagate and a foreign destination marker is never overwritten.
            (RemoteStatus::Broken(detail), _) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("source object is broken and must not be propagated: {detail}"),
                });
            }
            (_, RemoteStatus::Broken(detail)) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: format!("destination sibling is foreign/unparseable: {detail}"),
                });
            }
            (RemoteStatus::Absent, RemoteStatus::Absent) => missing.push(name.to_string()),
            // The enumeration lives at the source: a listed name without source bytes is an inconsistency.
            // The destination copy may stand, but mirroring never deletes (divergence is a human decision).
            (RemoteStatus::Absent, _) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail: "listed for mirroring but absent at the source; the destination copy stays untouched"
                        .into(),
                });
            }
            (
                RemoteStatus::Complete { digest: s, .. },
                RemoteStatus::Complete { digest: d, .. },
            ) => {
                if s == d {
                    actions.push(MirrorAction::Skip {
                        name,
                        digest: s.clone(),
                    });
                } else {
                    return Err(Verdict::Mismatch {
                        name: name.to_string(),
                        detail: format!(
                            "complete on both sides with different digests: source {s}, destination {d}"
                        ),
                    });
                }
            }
            // An absent or unfinished destination copy is completed: the finished source object is sent whole (PUTs are idempotent).
            (RemoteStatus::Complete { digest: s, .. }, _) => {
                actions.push(MirrorAction::Copy {
                    name,
                    size: src_size(&src),
                    expected: Some(s.clone()),
                    dst_digest: None,
                });
            }
            // The source carries no marker: the staged digest is computed from the bytes and compared against the destination marker.
            (RemoteStatus::Markerless { .. }, RemoteStatus::Complete { digest: d, .. }) => {
                actions.push(MirrorAction::Copy {
                    name,
                    size: src_size(&src),
                    expected: None,
                    dst_digest: Some(d.clone()),
                });
            }
            // Nothing verifiable on either side: refuse, exactly as up refuses.
            (RemoteStatus::Markerless { .. }, RemoteStatus::Markerless { .. }) => {
                return Err(Verdict::Mismatch {
                    name: name.to_string(),
                    detail:
                        "unverifiable on both sides: no marker anywhere; complete one copy first"
                            .into(),
                });
            }
            (RemoteStatus::Markerless { .. }, RemoteStatus::Absent) => {
                actions.push(MirrorAction::Copy {
                    name,
                    size: src_size(&src),
                    expected: None,
                    dst_digest: None,
                });
            }
        }
    }
    if !missing.is_empty() {
        return Err(Verdict::Missing { names: missing });
    }
    Ok(actions)
}

/// The HEAD-advertised size of a source state that has bytes.
fn src_size(status: &RemoteStatus) -> Option<u64> {
    match status {
        RemoteStatus::Complete { size, .. } | RemoteStatus::Markerless { size } => *size,
        _ => None,
    }
}

/// Run the plan: claim first and alone, then the rest under the worker semaphore.
///
/// Per-name failures land in `Summary.failed`; a diverging digest refuses the run as
/// [`Error::Mismatch`], a transport failure surfaces as its own error after retries.
///
/// # Errors
///
/// Returns [`Error::Mismatch`] on a source/destination digest divergence, [`Error::Misuse`] if a task panicked, and the first transport/auth/HTTP failure of the run.
pub(crate) async fn execute(
    client: Arc<NexusClient>,
    staging: PathBuf,
    src_url: String,
    dst_url: String,
    actions: Vec<MirrorAction>,
    claim: Option<ArtifactName>,
    workers: Arc<Semaphore>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut claim_action: Option<MirrorAction> = None;
    let mut rest: Vec<MirrorAction> = Vec::new();
    for action in actions {
        let is_claim = claim
            .as_ref()
            .is_some_and(|c| matches!(&action, MirrorAction::Copy { name, .. } if name == c));
        if is_claim {
            claim_action = Some(action);
        } else {
            rest.push(action);
        }
    }
    let mut summary = Summary::default();
    let mut mismatched: Vec<String> = Vec::new();
    let mut first_error: Option<Error> = None;
    if let Some(action) = claim_action {
        // Inline, not spawned: the claim must finish before anything else starts.
        // A failed claim aborts the run so the destination never gains bytes behind a version document that did not arrive.
        match copy_one(&client, &staging, &src_url, &dst_url, &action).await {
            Ok(Some(_)) => summary.uploaded += 1,
            Ok(None) => summary.skipped += 1,
            Err((name, Failure::Failed(e))) => {
                summary.failed = vec![name.to_string()];
                log::error!("mirror claim failed: {e}");
                client.progress().summary(&summary);
                return Err(e);
            }
            Err((name, Failure::Mismatch(detail))) => {
                summary.failed = vec![name.to_string()];
                client.progress().summary(&summary);
                return Err(Error::Mismatch {
                    name: format!("{name}: {detail}"),
                    detail: REFUSAL_DETAIL.to_owned(),
                });
            }
        }
    }
    for action in rest {
        match action {
            MirrorAction::Skip { name, .. } => {
                summary.skipped += 1;
                client
                    .progress()
                    .done(name.as_str(), Dir::Up, true, 0, None)
                    .await;
            }
            copy @ MirrorAction::Copy { .. } => {
                let permit = workers
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
                let client = client.clone();
                let staging = staging.clone();
                let src_url = src_url.clone();
                let dst_url = dst_url.clone();
                set.spawn(async move {
                    let _permit = permit;
                    copy_one(&client, &staging, &src_url, &dst_url, &copy).await
                });
            }
        }
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(Some(_))) => summary.uploaded += 1,
            // The destination proved complete and identical after all: nothing was pushed.
            Ok(Ok(None)) => summary.skipped += 1,
            Ok(Err((name, Failure::Failed(e)))) => {
                summary.failed.push(name.to_string());
                log::error!("mirror failed: {e}");
                first_error.get_or_insert(e);
            }
            Ok(Err((name, Failure::Mismatch(detail)))) => {
                summary.failed.push(name.to_string());
                mismatched.push(format!("{name}: {detail}"));
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
    }
    client.progress().summary(&summary);
    if !mismatched.is_empty() {
        return Err(Error::Mismatch {
            name: mismatched.join(", "),
            detail: REFUSAL_DETAIL.to_owned(),
        });
    }
    if let Some(e) = first_error {
        return Err(e);
    }
    Ok(summary)
}

/// The refusal detail for a diverging digest discovered during the copy.
const REFUSAL_DETAIL: &str = "mirrored content diverges from its sha-sibling";

/// Stage one name from the source, then push bytes and marker to the destination.
///
/// `Ok(Some(size))` names the bytes that landed at the destination.
/// `Ok(None)` means the destination already held exactly the staged digest: nothing was written there.
async fn copy_one(
    client: &NexusClient,
    staging: &Path,
    src_url: &str,
    dst_url: &str,
    action: &MirrorAction,
) -> Result<Option<u64>, (ArtifactName, Failure)> {
    let MirrorAction::Copy {
        name,
        size: size_hint,
        expected,
        dst_digest,
    } = action
    else {
        return Err((
            action_name(action),
            Failure::Failed(Error::misuse(
                "internal: a Skip never reaches the copy stage",
            )),
        ));
    };
    // Stage: Range-aware GET into the stable part file, digest check against the source
    // sibling when one exists, rename, marker written from the received bytes.
    // Markerless names stage from zero: there is nothing to verify a resume against.
    down::download_one(
        client,
        staging,
        src_url,
        name,
        *size_hint,
        expected.clone(),
        false,
        false,
    )
    .await?;
    // The staged sibling always names the staged bytes: the staging step writes it from the bytes it received.
    let sib = sibling_path(staging, name);
    let raw = std::fs::read_to_string(&sib)
        .map_err(|e| (name.clone(), Failure::Failed(Error::io(&sib, e))))?;
    let digest = sibling::parse_line(&raw).map(|p| p.digest).map_err(|e| {
        (
            name.clone(),
            Failure::Failed(Error::misuse(format!(
                "internal: the staged sibling does not parse: {e}"
            ))),
        )
    })?;
    // The destination proved complete at classification but the source had no marker:
    // the staged digest decides. Equal: the destination already holds these bytes.
    // Different: refuse, the destination is never overwritten (divergence is a human decision).
    if let Some(d) = dst_digest {
        if d == &digest {
            client
                .progress()
                .done(name.as_str(), Dir::Up, true, 0, None)
                .await;
            return Ok(None);
        }
        return Err((
            name.clone(),
            Failure::Mismatch(format!(
                "staged digest {digest} diverges from the destination marker {d}"
            )),
        ));
    }
    let staged = bytes_path(staging, name);
    let size = std::fs::metadata(&staged)
        .map(|m| m.len())
        .map_err(|e| (name.clone(), Failure::Failed(Error::io(&staged, e))))?;
    up::upload_one(client, staging, dst_url, name, size, Some(digest), false)
        .await
        .map_err(|e| (name.clone(), Failure::Failed(e)))?;
    client
        .progress()
        .done(name.as_str(), Dir::Up, false, size, Some(size))
        .await;
    Ok(Some(size))
}

/// The name of an action, for error paths that never fire in a mirror plan.
fn action_name(action: &MirrorAction) -> ArtifactName {
    match action {
        MirrorAction::Skip { name, .. } | MirrorAction::Copy { name, .. } => name.clone(),
    }
}

/// The version document to claim first, when the enumeration leads with it.
pub(crate) fn claim_first(actions: &[MirrorAction]) -> Option<ArtifactName> {
    match actions.first() {
        Some(MirrorAction::Copy { name, .. }) if name.as_str() == VERSION_DOCUMENT => {
            Some(name.clone())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(hex: &str) -> RemoteStatus {
        RemoteStatus::Complete {
            digest: Digest::from_hex_string(hex.to_owned()),
            size: None,
        }
    }

    fn states(pairs: &[(&str, RemoteStatus)]) -> BTreeMap<ArtifactName, RemoteStatus> {
        pairs
            .iter()
            .map(|(n, s)| (ArtifactName::parse(n).unwrap(), s.clone()))
            .collect()
    }

    fn ordered(list: &[&str]) -> Vec<ArtifactName> {
        list.iter()
            .map(|n| ArtifactName::parse(n).unwrap())
            .collect()
    }

    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn equal_complete_sides_skip() {
        let plan = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", complete(D1))]),
            states(&[("a.zip", complete(D1))]),
        )
        .unwrap();
        assert_eq!(
            plan,
            vec![MirrorAction::Skip {
                name: ArtifactName::parse("a.zip").unwrap(),
                digest: Digest::from_hex_string(D1.to_owned()),
            }]
        );
    }

    #[test]
    fn diverged_complete_refuses() {
        let err = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", complete(D1))]),
            states(&[("a.zip", complete(D2))]),
        )
        .unwrap_err();
        assert!(matches!(err, Verdict::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn unfinished_destination_copies_with_the_source_marker() {
        let plan = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", complete(D1))]),
            states(&[("a.zip", RemoteStatus::Markerless { size: Some(3) })]),
        )
        .unwrap();
        assert!(matches!(
            &plan[..],
            [MirrorAction::Copy {
                expected: Some(_),
                dst_digest: None,
                ..
            }]
        ));
    }

    #[test]
    fn markerless_source_completes_an_absent_destination() {
        let plan = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", RemoteStatus::Markerless { size: None })]),
            states(&[("a.zip", RemoteStatus::Absent)]),
        )
        .unwrap();
        assert!(matches!(
            &plan[..],
            [MirrorAction::Copy {
                expected: None,
                dst_digest: None,
                ..
            }]
        ));
    }

    #[test]
    fn markerless_source_against_complete_destination_defers_to_the_staged_digest() {
        let plan = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", RemoteStatus::Markerless { size: None })]),
            states(&[("a.zip", complete(D2))]),
        )
        .unwrap();
        assert!(matches!(
            &plan[..],
            [MirrorAction::Copy {
                expected: None,
                dst_digest: Some(_),
                ..
            }]
        ));
    }

    #[test]
    fn markerless_on_both_sides_refuses() {
        let err = classify(
            ordered(&["a.zip"]),
            states(&[("a.zip", RemoteStatus::Markerless { size: None })]),
            states(&[("a.zip", RemoteStatus::Markerless { size: Some(3) })]),
        )
        .unwrap_err();
        assert!(matches!(err, Verdict::Mismatch { .. }), "got {err:?}");
    }

    #[test]
    fn broken_source_and_foreign_destination_refuse() {
        let src = states(&[("a.zip", RemoteStatus::Broken("no".into()))]);
        let dst = states(&[("a.zip", RemoteStatus::Absent)]);
        assert!(classify(ordered(&["a.zip"]), src, dst).is_err());
        let src = states(&[("a.zip", complete(D1))]);
        let dst = states(&[("a.zip", RemoteStatus::Broken("no".into()))]);
        assert!(classify(ordered(&["a.zip"]), src, dst).is_err());
    }

    #[test]
    fn absent_everywhere_is_missing_only_without_other_refusals() {
        let src = states(&[("a.zip", RemoteStatus::Absent)]);
        let dst = states(&[("a.zip", RemoteStatus::Absent)]);
        assert!(matches!(
            classify(ordered(&["a.zip"]), src, dst),
            Err(Verdict::Missing { .. })
        ));
        let src = states(&[("a.zip", RemoteStatus::Absent), ("b.zip", complete(D1))]);
        let dst = states(&[("a.zip", RemoteStatus::Absent), ("b.zip", complete(D2))]);
        assert!(matches!(
            classify(ordered(&["a.zip", "b.zip"]), src, dst),
            Err(Verdict::Mismatch { .. })
        ));
    }

    #[test]
    fn staging_dir_separates_pairs_and_is_stable() {
        let a = staging_dir("http://a/", "http://b/");
        let b = staging_dir("http://a/", "http://c/");
        assert_ne!(a, b);
        assert_eq!(a, staging_dir("http://a/", "http://b/"));
        assert!(a.starts_with(std::env::temp_dir()));
    }
}
