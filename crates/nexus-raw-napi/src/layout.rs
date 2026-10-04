use napi::Result;
use napi_derive::napi;
use nexus_raw_core::{ChannelOutcome, ClearOutcome, Nxr};
use tokio::sync::mpsc;

use crate::error::js_error;
use crate::mapping;
use crate::opts::{split_common, NxrCommonOpts};
use crate::pump::{finish_pump, finish_pump_err, spawn_pump};
use crate::result::{NxrChannelSetResult, NxrPointClearResult};

// ---- L2 layout -------------------------------------------------------------

/// Read a channel ref: the token, or null when the channel is unset.
#[napi]
pub async fn channel_get(url: String, opts: Option<NxrCommonOpts>) -> Result<Option<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let token = nxr.channel_get(&url).await;
    drop(nxr);
    let token = match token {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(token)
}

/// Write a channel token with the optional forward-only guard.
#[napi]
pub async fn channel_set(
    url: String,
    token: String,
    if_forward: bool,
    opts: Option<NxrCommonOpts>,
) -> Result<NxrChannelSetResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let outcome = nxr.channel_set(&url, &token, if_forward).await;
    drop(nxr);
    let outcome = match outcome {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(match outcome {
        ChannelOutcome::Written { from } => NxrChannelSetResult {
            written: true,
            from,
            current: None,
        },
        ChannelOutcome::Skipped { current } => NxrChannelSetResult {
            written: false,
            from: None,
            current: Some(current),
        },
    })
}

/// DELETE a pointer file: absence is a normal outcome, like the CLI `point --clear`.
#[napi]
pub async fn point_clear(url: String, opts: Option<NxrCommonOpts>) -> Result<NxrPointClearResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let outcome = nxr.point_clear(&url).await;
    drop(nxr);
    let outcome = match outcome {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrPointClearResult {
        outcome: match outcome {
            ClearOutcome::Cleared => "cleared".into(),
            ClearOutcome::Absent => "absent".into(),
        },
    })
}

/// List the asset names the server search API reports for this directory, like `ls --assets`.
#[napi]
pub async fn ls_assets(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let names = nxr.ls_assets().await;
    drop(nxr);
    let names = match names {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(names.iter().map(|n| n.as_str().to_owned()).collect())
}

/// One immediate child of a raw directory: a folder or a file.
#[napi(object)]
pub struct NxrLsEntry {
    /// The child segment name.
    pub name: String,
    /// `"dir"` when the child has a subtree below it, `"file"` when it is a leaf.
    pub kind: String,
}

/// The immediate children of a raw directory URL, at any tree depth: folders first, then files.
/// A raw repository is an arbitrary tree; leaf `.sha256` siblings are hidden as derived data.
#[napi]
pub async fn ls_entries(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<NxrLsEntry>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let entries = nxr.ls_entries().await;
    drop(nxr);
    let entries = match entries {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(entries
        .into_iter()
        .map(|e| NxrLsEntry {
            name: e.name,
            kind: match e.kind {
                nexus_raw_core::EntryKind::Dir => "dir".to_owned(),
                nexus_raw_core::EntryKind::File => "file".to_owned(),
            },
        })
        .collect())
}

/// List the version tokens the server search API reports for this directory, like `ls`.
#[napi]
pub async fn ls_versions(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let versions = nxr.ls_versions().await;
    drop(nxr);
    let versions = match versions {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(versions)
}

/// One repository of a Nexus server, as the service REST API reports it.
#[napi(object)]
pub struct NxrRepoInfo {
    /// Repository name.
    pub name: String,
    /// Repository format (`raw`, `maven2`, ...), spelled as the server spells it.
    pub format: String,
    /// Repository kind (`hosted`, `proxy`, `group`).
    pub kind: String,
    /// Repository URL.
    pub url: String,
}

/// List the repositories of the server behind `url`: the service REST API (the management surface).
/// The URL may be the server root or any repository URL: both root to the same server.
/// Storage invariants never touch this endpoint; it exists for humans and panels.
#[napi]
pub async fn service_repos(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<NxrRepoInfo>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let repos = nxr.service_repos().await;
    drop(nxr);
    let repos = match repos {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(repos
        .into_iter()
        .map(|r| NxrRepoInfo {
            name: r.name,
            format: r.format,
            kind: r.kind,
            url: r.url,
        })
        .collect())
}
