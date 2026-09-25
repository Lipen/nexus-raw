//! The upload executor: PUT bytes → PUT marker, worker semaphore (protocol §6.1).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;

use crate::diff::Action;
use crate::error::Error;
use crate::events::{Dir, Summary};
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::{bytes_path, sibling_path};
use crate::transport::client::NexusClient;

/// Publish the artifacts: the claim is already checked and stored
/// (`put_claim_checked`).
///
/// Per-name failures (after retries) are collected into `Summary.failed`.
/// The Summary event goes out first, then [`Error::Incomplete`] is returned.
pub async fn execute(
    client: Arc<NexusClient>,
    dir: PathBuf,
    version: String,
    actions: Vec<Action>,
    workers: Arc<Semaphore>,
) -> Result<Summary, Error> {
    let mut set = tokio::task::JoinSet::new();
    let mut skipped = 0usize;
    for action in actions {
        match action {
            Action::Skip { name, .. } => {
                skipped += 1;
                client
                    .progress()
                    .done(name.as_str(), Dir::Up, true, 0, None)
                    .await;
            }
            Action::Upload { name, size } => {
                let permit = workers
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| Error::misuse(format!("worker semaphore closed: {e}")))?;
                let client = client.clone();
                let dir = dir.clone();
                let version = version.clone();
                set.spawn(async move {
                    let _permit = permit;
                    match upload_one(&client, &dir, &version, &name, size).await {
                        Ok(()) => Ok(name),
                        Err(e) => Err((name.to_string(), e)),
                    }
                });
            }
            Action::Download { name, .. } => {
                // A Download never appears in an up plan: the precheck requires Complete locally.
                return Err(Error::misuse(format!(
                    "internal: Download action {name} in an up plan"
                )));
            }
        }
    }
    let mut summary = Summary {
        skipped,
        ..Summary::default()
    };
    let mut failed: Vec<String> = Vec::new();
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(_)) => summary.uploaded += 1,
            Ok(Err((name, e))) => {
                failed.push(name);
                log::error!("up failed: {e}");
            }
            Err(e) => return Err(Error::misuse(format!("task panicked: {e}"))),
        }
    }
    client.progress().summary(&summary);
    if !failed.is_empty() {
        return Err(Error::Incomplete { names: failed });
    }
    Ok(summary)
}

/// PUT the bytes, then the canonical marker of the same name (§6.1.2).
async fn upload_one(
    client: &NexusClient,
    dir: &Path,
    version: &str,
    name: &ArtifactName,
    size: u64,
) -> Result<(), Error> {
    let bytes_url = client.object_url(version, name);
    let bytes = bytes_path(dir, name);
    client
        .upload_file((name.as_str(), Dir::Up), &bytes_url, &bytes, size)
        .await?;
    // The sibling was verified Complete by the diff.
    // Write the canonical line.
    let sib_file = sibling_path(dir, name);
    let raw = tokio::fs::read_to_string(&sib_file)
        .await
        .map_err(|e| Error::io(&sib_file, e))?;
    let parsed = sibling::parse_line(&raw).map_err(|e| Error::Mismatch {
        name: name.to_string(),
        detail: format!("local sibling turned unparseable before upload: {e}"),
    })?;
    let sib_url = format!("{bytes_url}.sha256");
    client
        .put_small(
            &sib_url,
            sibling::format_line(name.as_str(), &parsed.digest).into_bytes(),
        )
        .await?;
    client
        .progress()
        .done(name.as_str(), Dir::Up, false, size, Some(size))
        .await;
    Ok(())
}
