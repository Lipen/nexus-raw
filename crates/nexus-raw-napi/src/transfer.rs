use std::path::Path;

use napi::bindgen_prelude::Either;
use napi::Result;
use napi_derive::napi;
use nexus_raw_core::{ArtifactName, Mode, Nxr};
use tokio::sync::mpsc;

use crate::error::js_error;
use crate::input::{load_manifest, parse_names, resolve_enumeration};
use crate::mapping::{self, CommonOpts};
use crate::opts::{
    split_common, NxrAuth, NxrDownOpts, NxrMirrorOpts, NxrRmOpts, NxrUpOpts, NxrVerifyOpts,
};
use crate::pump::{finish_pump, finish_pump_err, spawn_pump};
use crate::result::{NxrPlan, NxrPlanAction, NxrRmPlan, NxrRmPlanAction, NxrSummary};

// ---- L1 transfer -----------------------------------------------------------

/// Upload a local directory.
///
/// With `dryRun` the promise resolves to the plan instead of a summary and nothing transfers, like `up --dry-run`.
#[napi]
pub async fn up(
    src_dir: String,
    dst_url: String,
    opts: Option<NxrUpOpts>,
) -> Result<Either<NxrSummary, NxrPlan>> {
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
    let cfg = mapping::build_config(&dst_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let claim = match &o.claim_first {
        Some(raw) => Some(ArtifactName::parse(raw).map_err(js_error)?),
        None => None,
    };
    let gen_markers = !o.no_sha.unwrap_or(false);
    // The pump starts before any network call: events fired during the manifest fetch belong to JS as much as the later ones.
    let pump = spawn_pump(rx, on_event);
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => match load_manifest(&nxr, spec).await {
            Ok(m) => Some(m.names),
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        },
        None => parsed_names,
    };
    if o.dry_run.unwrap_or(false) {
        let scanned = match names {
            Some(n) => n,
            None => match nxr.scan(Path::new(&src_dir)) {
                Ok(n) => n,
                Err(e) => {
                    drop(nxr);
                    return Err(finish_pump_err(pump, e).await);
                }
            },
        };
        let actions = match nxr
            .diff(Path::new(&src_dir), scanned, Mode::Up, gen_markers)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        };
        // The plan promise settles only after the events did, and the events end only when the facade's sender is gone.
        drop(nxr);
        finish_pump(pump).await?;
        return Ok(Either::B(NxrPlan {
            actions: actions.iter().map(NxrPlanAction::from).collect(),
        }));
    }
    let summary = nxr.up(Path::new(&src_dir), names, gen_markers, claim).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(Either::A(NxrSummary::from(&summary)))
}

/// Download a remote directory into a local one.
///
/// The enumeration source is mandatory: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// Otherwise the promise rejects like the CLI.
#[napi]
pub async fn down(
    src_url: String,
    dst_dir: String,
    opts: Option<NxrDownOpts>,
) -> Result<NxrSummary> {
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
    let cfg = mapping::build_config(&src_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    // The pump starts before any network call: events fired during the manifest fetch belong to JS as much as the later ones.
    // Every early return from here drops the facade before draining the pump — the pump ends only when the facade's sender is gone.
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json on the server and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    let summary = nxr
        .down(Path::new(&dst_dir), enum_src, o.fresh.unwrap_or(false))
        .await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}

/// Delete the enumerated names from the remote directory.
///
/// The enumeration source is mandatory, like `down`: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// Divergence is never checked: `rm` deletes names, not content.
/// With `dryRun` the promise resolves to the plan instead of a summary and nothing is deleted, like `rm --dry-run`.
#[napi]
pub async fn rm(src_url: String, opts: Option<NxrRmOpts>) -> Result<Either<NxrSummary, NxrRmPlan>> {
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
    let cfg = mapping::build_config(&src_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json on the server and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    if o.dry_run.unwrap_or(false) {
        let plan = nxr.rm_plan(enum_src).await;
        drop(nxr);
        let actions = match plan {
            Ok(a) => a,
            Err(e) => return Err(finish_pump_err(pump, e).await),
        };
        finish_pump(pump).await?;
        return Ok(Either::B(NxrRmPlan {
            actions: actions.iter().map(NxrRmPlanAction::from).collect(),
        }));
    }
    let summary = nxr.rm(enum_src).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(Either::A(NxrSummary::from(&summary)))
}

/// Pour enumerated names from a source repository into a destination one.
///
/// Two facades on one event stream: enumeration and bytes come from the source, the diff and the writes follow the destination.
/// The enumeration source is mandatory and lives at the source: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` there.
#[napi]
pub async fn mirror(
    src_url: String,
    dst_url: String,
    opts: Option<NxrMirrorOpts>,
) -> Result<NxrSummary> {
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
    let side_auth = |side: &Option<NxrAuth>| match side {
        Some(a) => CommonOpts {
            auth_user: Some(a.user.clone()),
            auth_pass: Some(a.pass.clone()),
            ..common.clone()
        },
        None => common.clone(),
    };
    let src_cfg = mapping::build_config(&src_url, &side_auth(&o.src_auth)).map_err(js_error)?;
    let dst_cfg = mapping::build_config(&dst_url, &side_auth(&o.dst_auth)).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    // Both facades build before anything moves: a bad destination URL is misuse, not a half-poured version.
    let src_nxr = Nxr::new(src_cfg, tx.clone()).map_err(js_error)?;
    let dst_nxr = Nxr::new(dst_cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &src_nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json at the source and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(src_nxr);
            drop(dst_nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    let summary = src_nxr.mirror(&dst_nxr, enum_src).await;
    // Both facades must die before the pump drains: each holds a sender clone.
    drop(src_nxr);
    drop(dst_nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}

/// Check local bytes, markers and digests. No network.
#[napi]
pub async fn verify(dir: String, opts: Option<NxrVerifyOpts>) -> Result<NxrSummary> {
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
    // verify is local-only: the base is a placeholder, like the CLI.
    let cfg = mapping::build_config("http://localhost/", &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: the pump starts before any manifest fetch.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => match load_manifest(&nxr, spec).await {
            Ok(m) => Some(m.names),
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        },
        None => parsed_names,
    };
    let summary = nxr.verify(Path::new(&dir), names).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}
