//! The upload executor: PUT bytes → PUT canonical marker, worker semaphore (§5.2).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::error::Error;
use crate::events::{Dir, Summary};
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::bytes_path;
use crate::sync::diff::Action;
use crate::transport::client::NexusClient;

/// Upload the artifacts: bytes first, then the marker of the same name.
///
/// `dir_url` is the normalized remote directory.
/// Per-name failures (after retries) are collected into `Summary.failed`.
/// The Summary event goes out first, then [`Error::Incomplete`] is returned.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    dir_url: String,
    actions: Vec<Action>,
    claim: Option<ArtifactName>,
    workers: Arc<Semaphore>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut skipped = 0usize;
    let mut claim_uploaded = 0usize;
    // Claim-first (§2): the named claim uploads alone, before any other name
    // starts. A failed or refused claim aborts the run so consumers never see
    // a version whose list disagrees with its bytes.
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
        if let Err(e) = upload_one(&client, &dir, &dir_url, &name, size, digest).await {
            client.progress().summary(&Summary {
                uploaded: 0,
                failed: vec![name.to_string()],
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
                let permit = workers
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
                let client = client.clone();
                let dir = dir.clone();
                let dir_url = dir_url.clone();
                set.spawn(async move {
                    let _permit = permit;
                    match upload_one(&client, &dir, &dir_url, &name, size, digest).await {
                        Ok(()) => Ok(name),
                        Err(e) => Err((name.to_string(), e)),
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
                failed.push(name);
                log::error!("up failed: {e}");
                if first_error.is_none()
                    && matches!(
                        e,
                        Error::Auth { .. } | Error::Transport { .. } | Error::Http { .. }
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
    // Transport-level failures surface as their own error (exit 3),
    // the failed names stay visible in the summary.
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

/// PUT the bytes, then the canonical marker of the same name (§5.2).
/// The marker strictly follows the bytes: a crash in between leaves a
/// Markerless object, which every reader refuses to trust.
async fn upload_one(
    client: &NexusClient,
    dir: &std::path::Path,
    dir_url: &str,
    name: &crate::model::name::ArtifactName,
    size: u64,
    digest: Option<crate::model::digest::Digest>,
) -> Result<(), Error> {
    let bytes_url = client.object_url(dir_url, name);
    let src = bytes_path(dir, name);
    client
        .upload_file((name.as_str(), Dir::Up), &bytes_url, &src, size)
        .await?;
    if let Some(d) = digest {
        let marker_url = client.sibling_url(dir_url, name);
        client
            .put_small(
                &marker_url,
                sibling::format_line(name.as_str(), &d).into_bytes(),
            )
            .await?;
    }
    client
        .progress()
        .done(name.as_str(), Dir::Up, false, size, Some(size))
        .await;
    Ok(())
}
