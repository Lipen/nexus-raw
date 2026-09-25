//! Pointer updates: read/write with the forward-only guard (§6.3).

use crate::error::Error;
use crate::model::name::{is_pointer_name, validate_version};
use crate::model::pointer;
use crate::transport::client::NexusClient;

/// The `point` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointOutcome {
    /// `from` is the previous value when it was readable.
    Written { from: Option<String> },
    Skipped { current: String },
}

/// PUT a pointer with the optional forward-only guard (`--if-newer`).
///
/// A current value that cannot be read (404, garbage) never blocks the write (§6.3).
pub async fn point(
    client: &NexusClient,
    name: &str,
    version: &str,
    if_newer: bool,
) -> Result<PointOutcome, Error> {
    if !is_pointer_name(name) {
        return Err(Error::misuse(format!(
            "pointer name must be one of latest|nightly, got {name:?}"
        )));
    }
    validate_version(version)?;
    let current = client.get_pointer(name).await?;
    if if_newer {
        if let Some(raw) = &current {
            if let Ok(cur) = pointer::parse_token(raw) {
                if pointer::version_ge(&cur, version) {
                    return Ok(PointOutcome::Skipped { current: cur });
                }
            }
        }
    }
    client.put_pointer(name, version).await?;
    Ok(PointOutcome::Written {
        from: current.and_then(|raw| pointer::parse_token(&raw).ok()),
    })
}
