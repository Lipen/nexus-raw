//! The L1 commands: up and down.

use std::path::Path;

use nexus_raw_core::{ArtifactName, Enumeration, Error};

use crate::cmd::{finish, load_manifest, make_ctx, parse_names, Ctx};
use crate::Cli;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn up(
    cli: &Cli,
    src: &Path,
    dst: &str,
    manifest: Option<&str>,
    claim: Option<ArtifactName>,
    no_sha: bool,
    dry_run: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, dst)?;
    // The renderer drains on every path: the events the run already emitted
    // must reach the output before the failure is reported.
    let result = run_up(&ctx, src, manifest, claim, no_sha, dry_run).await;
    finish(ctx).await;
    result
}

async fn run_up(
    ctx: &Ctx,
    src: &Path,
    manifest: Option<&str>,
    claim: Option<ArtifactName>,
    no_sha: bool,
    dry_run: bool,
) -> Result<(), Error> {
    let names = match manifest {
        Some(spec) => Some(load_manifest(&ctx.nxr, spec).await?.names),
        None => None,
    };
    if dry_run {
        let names = match names {
            Some(n) => Some(n),
            None => Some(ctx.nxr.scan(src)?),
        };
        let actions = ctx
            .nxr
            .diff(
                src,
                names.unwrap_or_default(),
                nexus_raw_core::Mode::Up,
                !no_sha,
            )
            .await?;
        for a in &actions {
            crate::cmd::print_line(ctx.json, plan_line(a), action_json(a));
        }
        return Ok(());
    }
    // The Summary event is the single source for totals; the renderer prints
    // it in both modes. Printing a second copy here raced the renderer.
    ctx.nxr.up(src, names, !no_sha, claim, None).await?;
    Ok(())
}

pub(crate) async fn down(
    cli: &Cli,
    src: &str,
    dst: &Path,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    fresh: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let result = run_down(&ctx, dst, manifest, names, ls, fresh).await;
    finish(ctx).await;
    result
}

async fn run_down(
    ctx: &Ctx,
    dst: &Path,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    fresh: bool,
) -> Result<(), Error> {
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
    ctx.nxr.down(dst, enum_src, fresh, None).await?;
    Ok(())
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
