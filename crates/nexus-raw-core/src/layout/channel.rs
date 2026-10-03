//! Channel refs: mutable token files with any name (spec §5.3).
//!
//! A channel is a plain object holding exactly one token line (`<token>\n`).

use crate::error::Error;
use crate::model::pointer;
use crate::transport::client::NexusClient;

/// The `channel set` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOutcome {
    /// `from` is the previous value when it was readable.
    Written { from: Option<String> },
    /// The forward-only guard kept the current value.
    Skipped { current: String },
}

/// Read a channel: None on 404, the token otherwise.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the underlying GET, and [`Error::Misuse`] when the body exceeds the small-object cap.
pub async fn get(client: &NexusClient, url: &str) -> Result<Option<String>, Error> {
    match client.get_small(url).await? {
        None => Ok(None),
        Some(raw) => pointer::parse_token(&String::from_utf8_lossy(&raw))
            .map(Some)
            .map_err(|e| Error::Mismatch {
                name: url.to_owned(),
                detail: format!("channel file does not parse: {e}"),
            }),
    }
}

/// Write a channel token with the optional forward-only guard (`--if-forward`).
///
/// A current value that cannot be read (404, garbage) never blocks the write.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `token` is not one line, and transport, auth or HTTP errors when reading the current value or writing the new one.
pub async fn set(
    client: &NexusClient,
    url: &str,
    token: &str,
    if_forward: bool,
) -> Result<ChannelOutcome, Error> {
    pointer::validate_token(token)
        .map_err(|e| Error::misuse(format!("channel token {token:?} is not one line: {e}")))?;
    let current = get(client, url).await?;
    if if_forward {
        if let Some(cur) = &current {
            if pointer::version_ge(cur, token) {
                return Ok(ChannelOutcome::Skipped {
                    current: cur.clone(),
                });
            }
        }
    }
    client
        .put_small(url, pointer::format_token(token).into_bytes())
        .await?;
    Ok(ChannelOutcome::Written { from: current })
}

/// The `point --clear` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClearOutcome {
    /// The pointer existed and is deleted.
    Cleared,
    /// The pointer was already absent: 404, and clearing stays idempotent.
    Absent,
}

/// DELETE a pointer file (`point --clear`, §5.4).
///
/// Clearing is not a comparison: there is no `--if-forward` semantics here.
/// An already-absent pointer is a normal outcome, so the call is idempotent.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the DELETE; a 403/405 surfaces as [`Error::ReadOnly`].
pub async fn clear(client: &NexusClient, url: &str) -> Result<ClearOutcome, Error> {
    Ok(match client.delete_url(url).await? {
        crate::transport::client::DeleteOutcome::Deleted => ClearOutcome::Cleared,
        crate::transport::client::DeleteOutcome::Missing => ClearOutcome::Absent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn set_refuses_multiline_tokens() {
        // No network needed: validation fires first.
        let client = dummy_client().await;
        let err = set(&client, "http://mock/latest", "two\nlines", false)
            .await
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    async fn dummy_client() -> NexusClient {
        use crate::config::Config;
        use crate::events::Progress;
        use std::time::Duration;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cfg = Config {
            base: "http://mock/".into(),
            tls_insecure: false,
            workers: 1,
            retry_attempts: 1,
            connect_timeout: Duration::from_secs(1),
            stall_timeout: Duration::from_secs(1),
            auth: None,
        };
        NexusClient::new(&cfg, Progress::new(tx)).unwrap()
    }
}
