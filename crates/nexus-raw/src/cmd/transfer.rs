//! The L1 commands: up, down and mirror.

use std::path::Path;

use nexus_raw_core::{ArtifactName, Enumeration, Error, Nxr};

use crate::cmd::{enumeration_source, finish, load_manifest, make_ctx, make_nxr, parse_names, Ctx};
use crate::render::{Mode, Session};
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
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
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
            crate::cmd::print_line(ctx.json, &plan_line(a), &action_json(a));
        }
        return Ok(());
    }
    // The Summary event is the single source for totals: the renderer prints it in both modes.
    // Printing a second copy here raced the renderer.
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
    let enum_src = enumeration_source(ctx, manifest, names, ls).await?;
    ctx.nxr.down(dst, enum_src, fresh, None).await?;
    Ok(())
}

/// Delete the enumerated names from a remote directory: marker first, then bytes.
pub(crate) async fn rm(
    cli: &Cli,
    src: &str,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let result = run_rm(&ctx, manifest, names, ls, dry_run).await;
    finish(ctx).await;
    result
}

async fn run_rm(
    ctx: &Ctx,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
) -> Result<(), Error> {
    let enum_src = enumeration_source(ctx, manifest, names, ls).await?;
    if dry_run {
        for a in ctx.nxr.rm_plan(enum_src).await? {
            crate::cmd::print_line(ctx.json, &rm_plan_line(&a), &rm_action_json(&a));
        }
        return Ok(());
    }
    ctx.nxr.rm(enum_src).await?;
    Ok(())
}

fn rm_plan_line(a: &nexus_raw_core::RmAction) -> String {
    match a {
        nexus_raw_core::RmAction::Remove { name, .. } => format!("rm {name}"),
        nexus_raw_core::RmAction::Missing { name } => format!("missing {name}"),
    }
}

fn rm_action_json(a: &nexus_raw_core::RmAction) -> serde_json::Value {
    match a {
        nexus_raw_core::RmAction::Remove { name, size } => {
            serde_json::json!({"action": "rm", "name": name.to_string(), "size": size})
        }
        nexus_raw_core::RmAction::Missing { name } => {
            serde_json::json!({"action": "missing", "name": name.to_string()})
        }
    }
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

/// Mirror: pour enumerated names from a source repository into a destination one.
///
/// Two facades, one session: enumeration and bytes come from the source, the diff and
/// the writes follow the destination.
pub(crate) async fn mirror(
    cli: &Cli,
    src: &str,
    dst: &str,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
) -> Result<(), Error> {
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    // Both facades build before anything moves: a bad destination URL is misuse, not a half-poured version.
    let src_nxr = make_nxr(cli, src, session.sender())?;
    let dst_nxr = make_nxr(cli, dst, session.sender())?;
    let result = run_mirror(&src_nxr, &dst_nxr, manifest, names, ls).await;
    // Both facades must die before the renderer drains: each holds a sender clone.
    drop(src_nxr);
    drop(dst_nxr);
    session.finish().await;
    result
}

async fn run_mirror(
    src: &Nxr,
    dst: &Nxr,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
) -> Result<(), Error> {
    // Enumeration only from the source: the same three sources as down.
    let enum_src = if ls {
        Enumeration::Search
    } else if let Some(spec) = manifest {
        Enumeration::Manifest(load_manifest(src, spec).await?)
    } else if !names.is_empty() {
        Enumeration::Names(parse_names(names)?)
    } else {
        // The convention: a manifest.json at the source directory URL.
        match src.manifest_at_base().await? {
            Some(m) => Enumeration::Manifest(m),
            None => {
                return Err(Error::Enumerate {
                    url: src.base().to_owned(),
                    reason: "no manifest.json at the source and no --manifest/--name/--ls given"
                        .into(),
                })
            }
        }
    };
    src.mirror(dst, enum_src).await?;
    Ok(())
}
