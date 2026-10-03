//! The deletion executor: DELETE the marker, then the bytes of every name (§5.4).

use std::sync::Arc;

use crate::error::Error;
use crate::events::Summary;
use crate::model::name::ArtifactName;
use crate::transport::client::{DeleteOutcome, NexusClient};

/// One planned deletion, as `rm --dry-run` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RmAction {
    /// The remote copy exists: marker then bytes would be DELETEd.
    Remove {
        name: ArtifactName,
        size: Option<u64>,
    },
    /// Already absent: a real run would see only 404s.
    Missing { name: ArtifactName },
}

/// DELETE the marker, then the bytes, of every name in order.
///
/// The order inside a name is the reverse of publishing: the marker goes first,
/// so nobody ever sees a complete object mid-delete.
/// The order between names is free, and the walk is deliberately sequential:
/// the first refusal (read-only repository) or transport failure stops the run
/// before the next request, so a refusal has deleted nothing.
/// Divergence is never checked: names are deleted, not compared (§5.4).
///
/// 404 counts as done: deletion is idempotent.
/// The Summary event goes out before the error returns, and `failed` names the run stopper.
pub async fn execute(
    client: Arc<NexusClient>,
    dir_url: String,
    names: Vec<ArtifactName>,
) -> Result<Summary, Error> {
    let mut summary = Summary::default();
    for name in &names {
        client.progress().removing(name.as_str()).await;
        let mut touched = false;
        // The marker URL strictly precedes the bytes URL of the same name.
        for url in [
            client.sibling_url(&dir_url, name),
            client.object_url(&dir_url, name),
        ] {
            match client.delete_url(&url).await {
                Ok(DeleteOutcome::Deleted) => touched = true,
                Ok(DeleteOutcome::Missing) => {}
                Err(e) => {
                    summary.failed.push(name.to_string());
                    client.progress().summary(&summary);
                    return Err(e);
                }
            }
        }
        if touched {
            summary.removed += 1;
            client.progress().removed(name.as_str()).await;
        } else {
            summary.skipped += 1;
            client.progress().missing(name.as_str()).await;
        }
    }
    client.progress().summary(&summary);
    Ok(summary)
}
