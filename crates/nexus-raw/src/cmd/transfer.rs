//! The L1 commands: up and down.

use std::path::Path;

use nexus_raw_core::{Enumeration, Error};

use crate::cmd::{finish, load_manifest, make_ctx, parse_names, Ctx};
use crate::Cli;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn up(
    cli: &Cli,
    src: &Path,
    dst: &str,
    manifest: Option<&str>,
    no_sha: bool,
    dry_run: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, dst)?;
    let names = match manifest {
        Some(spec) => {
            let m = load_manifest(&ctx.nxr, spec).await?;
            Some(m.names)
        }
        None => None,
    };
    if dry_run {
        let names = match names {
            Some(n) => Some(n),
            None => Some(ctx.nxr.scan(src)?),
        };
        let actions = match ctx
            .nxr
            .diff(
                src,
                names.unwrap_or_default(),
                nexus_raw_core::Mode::Up,
                !no_sha,
            )
            .await
        {
            Ok(a) => a,
            Err(e) => {
                finish(ctx).await;
                return Err(e);
            }
        };
        for a in &actions {
            crate::cmd::print_line(ctx.json, plan_line(a), action_json(a));
        }
        finish(ctx).await;
        return Ok(());
    }
    let summary = ctx.nxr.up(src, names, !no_sha, None).await?;
    report_summary(&ctx, &summary, "up");
    finish(ctx).await;
    Ok(())
}

pub(crate) async fn down(
    cli: &Cli,
    src: &str,
    dst: &Path,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    cont: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let enum_src = if ls {
        Enumeration::Search
    } else if let Some(spec) = manifest {
        Enumeration::Manifest(load_manifest(&ctx.nxr, spec).await?)
    } else if !names.is_empty() {
        Enumeration::Names(parse_names(names)?)
    } else {
        // The convention: a manifest.json in the version directory.
        match ctx.nxr.manifest_at_base().await? {
            Some(m) => Enumeration::Manifest(m),
            None => {
                return Err(Error::Enumerate {
                    url: ctx.nxr.base().to_owned(),
                    reason: "no manifest.json on the server and no --manifest/--name/--ls given"
                        .into(),
                })
            }
        }
    };
    let summary = ctx.nxr.down(dst, enum_src, cont, None).await?;
    report_summary(&ctx, &summary, "down");
    finish(ctx).await;
    Ok(())
}

fn report_summary(ctx: &Ctx, summary: &nexus_raw_core::Summary, what: &str) {
    if ctx.json {
        return; // The Summary event already went out as NDJSON.
    }
    let mut line = format!(
        "{what}: {} sent, {} fetched, {} skipped",
        summary.uploaded, summary.downloaded, summary.skipped
    );
    if !summary.failed.is_empty() {
        line.push_str(&format!(", FAILED: {}", summary.failed.join(", ")));
    }
    println!("{line}");
}

fn plan_line(a: &nexus_raw_core::Action) -> String {
    match a {
        nexus_raw_core::Action::Skip { name, .. } => format!("skip {name}"),
        nexus_raw_core::Action::Upload { name, .. } => format!("upload {name}"),
        nexus_raw_core::Action::Download { name, .. } => format!("download {name}"),
    }
}

fn action_json(a: &nexus_raw_core::Action) -> serde_json::Value {
    match a {
        nexus_raw_core::Action::Skip { name, .. } => {
            serde_json::json!({"action": "skip", "name": name.to_string()})
        }
        nexus_raw_core::Action::Upload { name, size, .. } => {
            serde_json::json!({"action": "upload", "name": name.to_string(), "size": size})
        }
        nexus_raw_core::Action::Download { name, size, .. } => {
            serde_json::json!({"action": "download", "name": name.to_string(), "size": size})
        }
    }
}
