//! The upload executor: PUT bytes → PUT canonical marker, worker semaphore (§5.2).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::Error;
use crate::events::{Dir, Failure, Summary};
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path, LocalStatus};
use crate::sync::diff::Action;
use crate::transport::aimd::WorkerLimit;
use crate::transport::client::NexusClient;

/// Upload the artifacts: bytes first, then the marker of the same name.
///
/// `dir_url` is the normalized remote directory.
/// Per-name failures (after retries) are collected into `Summary.failed`.
/// The Summary event goes out first, then [`Error::Incomplete`] is returned.
///
/// # Errors
///
/// Returns the first transport/auth/HTTP failure of the run, [`Error::Incomplete`] listing the failed names, and [`Error::Misuse`] for internal plan-shape violations.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    dir_url: String,
    actions: Vec<Action>,
    claim: Option<ArtifactName>,
    workers: Arc<WorkerLimit>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut skipped = 0usize;
    let mut claim_uploaded = 0usize;
    // Claim-first (§2): the named claim uploads alone, before any other name starts.
    // A failed or refused claim aborts the run so consumers never see a version whose list disagrees with its bytes.
    let mut claim_action: Option<Action> = None;
    let mut rest: Vec<Action> = Vec::new();
    for action in actions {
        let is_claim = claim
            .as_ref()
            .is_some_and(|c| matches!(&action, Action::Upload { name, .. } if name == c));
        if is_claim {
            claim_action = Some(action);
        } else {
            rest.push(action);
        }
    }
    if let Some(Action::Upload { name, size, digest }) = claim_action {
        // Inline, not spawned: the claim must finish before anything starts.
        if let Err(e) = upload_one(&client, &dir, &dir_url, &name, size, digest, true).await {
            let failure = Failure {
                name: name.to_string(),
                code: e.exit_code(),
            };
            client.progress().summary(&Summary {
                uploaded: 0,
                failed: vec![failure.name.clone()],
                failures: vec![failure],
                ..Summary::default()
            });
            return Err(e);
        }
        claim_uploaded += 1;
    }
    for action in rest {
        match action {
            Action::Skip { name, .. } => {
                skipped += 1;
                client
                    .progress()
                    .done(name.as_str(), Dir::Up, true, 0, None)
                    .await;
            }
            Action::Upload { name, size, digest } => {
                // The AIMD budget admits the task under the current limit (§7):
                // the permit settles with the task's outcome when it finishes.
                let permit = workers.acquire().await.map_err(Error::misuse)?;
                let client = client.clone();
                let dir = dir.clone();
                let dir_url = dir_url.clone();
                let workers = workers.clone();
                set.spawn(async move {
                    let result =
                        upload_one(&client, &dir, &dir_url, &name, size, digest, true).await;
                    // The permit is dropped first, so a decrease that landed
                    // mid-task reclaims it in `release_*` as owed.
                    drop(permit);
                    if result.is_ok() {
                        workers.release_success();
                        Ok(name)
                    } else {
                        let e = result.err().unwrap_or_else(|| {
                            Error::misuse("internal: a failed upload carried no error")
                        });
                        workers.release_failure();
                        Err((name.to_string(), e))
                    }
                });
            }
            // A Download never appears in an up plan: classify refuses those shapes.
            Action::Download { name, .. } => {
                return Err(Error::misuse(format!(
                    "internal: Download action {name} in an up plan"
                )));
            }
        }
    }
    let mut summary = Summary {
        skipped,
        uploaded: claim_uploaded,
        ..Summary::default()
    };
    let mut failed: Vec<String> = Vec::new();
    let mut first_error: Option<Error> = None;
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(_)) => summary.uploaded += 1,
            Ok(Err((name, e))) => {
                summary.failures.push(Failure {
                    name: name.clone(),
                    code: e.exit_code(),
                });
                failed.push(name);
                log::error!("up failed: {e}");
                if first_error.is_none()
                    && matches!(
                        e,
                        Error::Auth { .. }
                            | Error::Transport { .. }
                            | Error::Http { .. }
                            | Error::ReadOnly { .. }
                    )
                {
                    first_error = Some(e);
                }
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
    }
    summary.failed = failed;
    client.progress().summary(&summary);
    // A failure that is not a per-name data verdict surfaces as its own error:
    // auth and transport are exit 3, a refused write (a group repository, a host
    // without PUT) is exit 1. The failed names stay visible in the summary.
    if let Some(e) = first_error {
        return Err(e);
    }
    if !summary.failed.is_empty() {
        return Err(Error::Incomplete {
            names: summary.failed.clone(),
        });
    }
    Ok(summary)
}

/// The TOCTOU guard: the marker blesses only the bytes it was computed from.
/// A file rewritten between classification and upload carries a different
/// digest on the wire: publishing the classified marker would bless foreign
/// bytes behind exit 0.
pub(crate) fn ensure_matches(
    name: &crate::model::name::ArtifactName,
    classified: &crate::model::digest::Digest,
    wire: &crate::model::digest::Digest,
) -> Result<(), Error> {
    if classified != wire {
        return Err(Error::Mismatch {
            name: name.to_string(),
            detail: format!(
                "the file changed during the upload: classified as {classified}, the wire carried {wire}"
            ),
        });
    }
    Ok(())
}

/// PUT the bytes, then the canonical marker of the same name (§5.2).
/// The marker strictly follows the bytes: a crash in between leaves a Markerless object, which every reader refuses to trust.
/// `emit_done == false` hands the settle event to the caller: the mirror reports one done line for the whole copy, not one per phase.
pub(crate) async fn upload_one(
    client: &NexusClient,
    dir: &std::path::Path,
    dir_url: &str,
    name: &crate::model::name::ArtifactName,
    size: u64,
    digest: Option<crate::model::digest::Digest>,
    emit_done: bool,
) -> Result<(), Error> {
    let bytes_url = client.object_url(dir_url, name);
    let src = bytes_path(dir, name);
    let wire = client
        .upload_file((name.as_str(), Dir::Up), &bytes_url, &src, size)
        .await?;
    // The wire digest is what actually crossed the network: a file rewritten
    // between classification and upload diverges here, and the marker must
    // never bless bytes it does not describe (§5.2, the TOCTOU guard).
    if let Some(d) = digest {
        ensure_matches(name, &d, &wire)?;
        let marker_url = client.sibling_url(dir_url, name);
        client
            .put_small(
                &marker_url,
                sibling::format_line(name.as_str(), &d).into_bytes(),
            )
            .await?;
    }
    if emit_done {
        client
            .progress()
            .done(name.as_str(), Dir::Up, false, size, Some(size))
            .await;
    }
    Ok(())
}

/// Give every markerless local file its sibling: hash the bytes, write the canonical marker (§5.2 "markers are mandatory where we write").
/// Broken markers refuse early.
/// The file is never touched.
pub(crate) async fn generate_markers(
    dir: &Path,
    locals: &mut [(ArtifactName, LocalStatus)],
) -> Result<(), Error> {
    let dir = dir.to_owned();
    let mut jobs: Vec<(ArtifactName, LocalStatus)> = Vec::new();
    for (name, st) in locals.iter_mut() {
        match st {
            LocalStatus::Broken(detail) => {
                return Err(Error::Mismatch {
                    name: name.to_string(),
                    detail: format!("local object is broken and must not be overwritten: {detail}"),
                })
            }
            LocalStatus::Markerless => jobs.push((name.clone(), st.clone())),
            _ => {}
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }
    let made = tokio::task::spawn_blocking(move || {
        let mut made: Vec<(ArtifactName, crate::model::digest::Digest)> = Vec::new();
        for (name, _) in jobs {
            let bytes = crate::model::state::bytes_path(&dir, &name);
            let d = crate::model::digest::sha256_file(&bytes).map_err(|e| Error::io(&bytes, e))?;
            let sib = sibling_path(&dir, &name);
            // The marker path derives from the object name: never follow a symlink planted there (same rule as the download side).
            let mut f = crate::transport::client::write_options_blocking(false)
                .open(&sib)
                .map_err(|e| Error::io(&sib, e))?;
            use std::io::Write as _;
            f.write_all(crate::model::sibling::format_line(name.as_str(), &d).as_bytes())
                .map_err(|e| Error::io(&sib, e))?;
            made.push((name, d));
        }
        Ok::<Vec<_>, Error>(made)
    })
    .await
    .map_err(|e| Error::misuse(format!("task panicked: {e}")))??;
    let made: std::collections::HashMap<_, _> = made.into_iter().collect();
    for (name, st) in locals.iter_mut() {
        if let Some(d) = made.get(name) {
            *st = LocalStatus::Complete(d.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ensure_matches;
    use crate::model::digest::Digest;
    use crate::model::name::ArtifactName;

    #[test]
    fn the_guard_refuses_a_wire_divergence() {
        let name = ArtifactName::parse("a.zip").unwrap();
        let classified = Digest::from_hex(&"a".repeat(64)).unwrap();
        let wire = Digest::from_hex(&"b".repeat(64)).unwrap();
        let err = ensure_matches(&name, &classified, &wire).unwrap_err();
        assert!(
            err.to_string().contains("changed during the upload"),
            "the message must name the race: {err}"
        );
        // The matching pair passes: the happy path is untouched.
        ensure_matches(&name, &classified, &classified).unwrap();
    }
}
