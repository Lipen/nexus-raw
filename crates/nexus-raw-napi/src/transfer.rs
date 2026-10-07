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
    split_common, NxrAuth, NxrDiffOpts, NxrDownOpts, NxrMirrorOpts, NxrRmOpts, NxrUpOpts,
    NxrVerifyOpts,
};
use crate::pump::{finish_pump, finish_pump_err, spawn_pump};
use crate::result::{
    NxrDeltaReport, NxrPlan, NxrPlanAction, NxrRmPlan, NxrRmPlanAction, NxrSummary,
};

// ---- L1 transfer -----------------------------------------------------------

/// Upload a local directory.
///
/// With `dryRun` the promise resolves to the plan instead of a summary and nothing transfers, like `up --plan`.
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

/// Compare a local directory against a remote one: the delta report (`nxr diff`).
///
/// Read-only on both sides: the promise resolves to the report whatever the delta is, the verdict stays the caller's.
/// Every scanned or enumerated name lands in exactly one entry: `same`, `missing-local`, `missing-remote` or `diverged`.
/// The enumeration source is mandatory, like `down`: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` at the directory URL.
#[napi]
pub async fn diff(
    local_dir: String,
    remote_url: String,
    opts: Option<NxrDiffOpts>,
) -> Result<NxrDeltaReport> {
    // The CLI refuses a missing local directory before any network: same order here.
    if !Path::new(&local_dir).is_dir() {
        return Err(js_error(nexus_raw_core::Error::Misuse(format!(
            "not a directory: {local_dir}"
        ))));
    }
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
    let cfg = mapping::build_config(&remote_url, &common).map_err(js_error)?;
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
    let prefixes = o.prefixes.unwrap_or_default();
    let enum_src = match enum_src.with_prefixes(&prefixes) {
        Ok(v) => v,
        Err(e) => {
            drop(nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    let report = nxr.delta(Path::new(&local_dir), enum_src).await;
    drop(nxr);
    let entries = match report {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrDeltaReport::from(&entries[..]))
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

#[cfg(test)]
mod tests {
    use mock_nexus::{MockNexus, Scenario};
    use sha2::{Digest as _, Sha256};

    use super::diff;

    /// The sha256 hex of the bytes, lowercase, as the markers carry it.
    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The sha256sum-style marker line, exactly what the CLI fixtures seed.
    fn marker_line(name: &str, bytes: &[u8]) -> String {
        format!("{}  {name}\n", sha256_hex(bytes))
    }

    /// One name per delta state against an in-process mock-nexus.
    /// `a.zip` is same, `b.zip` missing-local, `c.zip` missing-remote, `d.zip` diverged;
    /// the conventional `manifest.json` at the directory URL enumerates all four.
    fn seed_four_states() -> (MockNexus, tempfile::TempDir) {
        let srv = MockNexus::start(Scenario::Atomic).expect("mock nexus starts");
        let local = tempfile::tempdir().expect("tempdir");
        let alpha = b"alpha bytes\n";
        let beta = b"beta bytes!!\n";
        // same: equal bytes on both sides, the remote copy carries its marker.
        std::fs::write(local.path().join("a.zip"), alpha).unwrap();
        srv.insert("1.14.0/a.zip", alpha);
        srv.insert(
            "1.14.0/a.zip.sha256",
            marker_line("a.zip", alpha).as_bytes(),
        );
        // missing-local: the storage holds the name, the local directory does not.
        srv.insert("1.14.0/b.zip", beta);
        srv.insert("1.14.0/b.zip.sha256", marker_line("b.zip", beta).as_bytes());
        // missing-remote: a local-only name, markerless on purpose: it still hashes on the fly.
        std::fs::write(local.path().join("c.zip"), alpha).unwrap();
        // diverged: both sides hold the name behind different bytes, both marked.
        std::fs::write(local.path().join("d.zip"), beta).unwrap();
        std::fs::write(
            local.path().join("d.zip.sha256"),
            marker_line("d.zip", beta).as_bytes(),
        )
        .unwrap();
        srv.insert("1.14.0/d.zip", alpha);
        srv.insert(
            "1.14.0/d.zip.sha256",
            marker_line("d.zip", alpha).as_bytes(),
        );
        srv.insert(
            "1.14.0/manifest.json",
            br#"{"schema_version":1,"version":"1.14.0","artifacts":["a.zip","b.zip","c.zip","d.zip"]}"#,
        );
        (srv, local)
    }

    #[tokio::test]
    async fn diff_reports_the_four_states() {
        let (srv, local) = seed_four_states();
        let url = format!("{}1.14.0/", srv.base_url());
        let report = diff(local.path().to_str().unwrap().to_owned(), url, None)
            .await
            .expect("the delta report resolves");
        assert_eq!(report.count, 4, "one entry per name in the union");
        let shape: Vec<(&str, &str)> = report
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.state.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                ("a.zip", "same"),
                ("b.zip", "missing-local"),
                ("c.zip", "missing-remote"),
                ("d.zip", "diverged"),
            ],
            "one entry per state, in name order"
        );
        let alpha_hex = sha256_hex(b"alpha bytes\n");
        let beta_hex = sha256_hex(b"beta bytes!!\n");
        // same: the shared digest travels, the side objects stay empty.
        let same = &report.entries[0];
        assert_eq!(same.digest.as_deref(), Some(alpha_hex.as_str()));
        assert!(same.local.is_none() && same.remote.is_none());
        assert!(same.size.is_none() && same.sha.is_none());
        // missing-local: only the storage side speaks.
        let missing_local = &report.entries[1];
        let remote = missing_local.remote.as_ref().unwrap();
        assert_eq!(remote.digest.as_deref(), Some(beta_hex.as_str()));
        assert_eq!(remote.size, Some(13.0));
        assert!(missing_local.local.is_none() && missing_local.digest.is_none());
        // missing-remote: only the local side speaks, hashed on the fly.
        let missing_remote = &report.entries[2];
        let local_side = missing_remote.local.as_ref().unwrap();
        assert_eq!(local_side.digest.as_deref(), Some(alpha_hex.as_str()));
        assert_eq!(local_side.size, Some(12.0));
        assert!(missing_remote.remote.is_none() && missing_remote.digest.is_none());
        // diverged: both sides speak, both dimensions provably differ.
        let diverged = &report.entries[3];
        assert_eq!(
            diverged.local.as_ref().unwrap().digest.as_deref(),
            Some(beta_hex.as_str())
        );
        assert_eq!(
            diverged.remote.as_ref().unwrap().digest.as_deref(),
            Some(alpha_hex.as_str())
        );
        assert!(diverged.size == Some(true) && diverged.sha == Some(true));
    }
}
