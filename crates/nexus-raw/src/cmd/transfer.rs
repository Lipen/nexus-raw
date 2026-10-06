//! The L1 commands: up, down and mirror.

use std::path::Path;

use nexus_raw_core::{ArtifactName, Enumeration, Error, Nxr};

use crate::cmd::{
    enumeration_source, finish, load_manifest, make_ctx, make_nxr_with, parse_names, Ctx,
};
use crate::render::{Mode, Session};
use crate::Cli;

/// The mirror pair with per-side credential overrides.
pub(crate) struct MirrorArgs<'a> {
    pub(crate) src: &'a str,
    pub(crate) dst: &'a str,
    pub(crate) src_user: Option<&'a str>,
    pub(crate) dst_user: Option<&'a str>,
}

pub(crate) async fn up(
    cli: &Cli,
    src: &Path,
    dst: &str,
    manifest: Option<&str>,
    claim: Option<ArtifactName>,
    no_sha: bool,
    plan: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, dst)?;
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
    let result = run_up(&ctx, src, manifest, claim, no_sha, plan).await;
    finish(ctx).await;
    result
}

async fn run_up(
    ctx: &Ctx,
    src: &Path,
    manifest: Option<&str>,
    claim: Option<ArtifactName>,
    no_sha: bool,
    plan: bool,
) -> Result<(), Error> {
    let names = match manifest {
        Some(spec) => Some(load_manifest(&ctx.nxr, spec).await?.names),
        None => None,
    };
    if plan {
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
    ctx.nxr.up(src, names, !no_sha, claim).await?;
    Ok(())
}

// The signature is the CLI surface: every argument is a user flag of `nxr down`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn down(
    cli: &Cli,
    src: &str,
    dst: &Path,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    fresh: bool,
    plan: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let enum_src = enumeration_source(&ctx, manifest, names, ls)
        .await?
        .with_prefixes(prefixes)?;
    let result = run_down(&ctx, dst, enum_src, fresh, plan).await;
    finish(ctx).await;
    result
}

async fn run_down(
    ctx: &Ctx,
    dst: &Path,
    enum_src: Enumeration,
    fresh: bool,
    plan: bool,
) -> Result<(), Error> {
    if plan {
        let names = ctx.nxr.enumerate(enum_src).await?;
        let actions = ctx
            .nxr
            .diff(dst, names, nexus_raw_core::Mode::Down, false)
            .await?;
        for a in &actions {
            crate::cmd::print_line(ctx.json, &plan_line(a), &action_json(a));
        }
        return Ok(());
    }
    ctx.nxr.down(dst, enum_src, fresh).await?;
    Ok(())
}

/// The delta of a local directory against a remote one (`nxr diff`).
///
/// Read-only on both sides: the verdict travels as the exit code, not as an error.
pub(crate) async fn diff(
    cli: &Cli,
    local: &Path,
    src: &str,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    prefixes: &[String],
) -> Result<u8, Error> {
    if !local.is_dir() {
        return Err(Error::Misuse(format!(
            "not a directory: {}",
            local.display()
        )));
    }
    let ctx = make_ctx(cli, src)?;
    let result = run_diff(&ctx, local, manifest, names, ls, prefixes).await;
    finish(ctx).await;
    result
}

async fn run_diff(
    ctx: &Ctx,
    local: &Path,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    prefixes: &[String],
) -> Result<u8, Error> {
    let enum_src = enumeration_source(ctx, manifest, names, ls)
        .await?
        .with_prefixes(prefixes)?;
    let entries = ctx.nxr.delta(local, enum_src).await?;
    for e in &entries {
        crate::cmd::print_line(ctx.json, &delta_line(e), &delta_json(e));
    }
    let differs = entries
        .iter()
        .any(|e| !matches!(e, nexus_raw_core::Delta::Same { .. }));
    Ok(u8::from(differs))
}

/// The human line of one delta entry.
fn delta_line(d: &nexus_raw_core::Delta) -> String {
    match d {
        nexus_raw_core::Delta::Same { name, .. } => format!("same {name}"),
        nexus_raw_core::Delta::MissingLocal { name, .. } => format!("missing-local {name}"),
        nexus_raw_core::Delta::MissingRemote { name, .. } => format!("missing-remote {name}"),
        nexus_raw_core::Delta::Diverged {
            name, size, sha, ..
        } => {
            let mut why = Vec::new();
            if *size {
                why.push("size");
            }
            if *sha {
                why.push("sha");
            }
            let tag = if why.is_empty() {
                "unverifiable".to_owned()
            } else {
                why.join("+")
            };
            format!("diverged {name} ({tag})")
        }
    }
}

/// The JSON shape of one side's facts: `null` when a fact is unknown.
fn delta_side_json(s: &nexus_raw_core::Side) -> serde_json::Value {
    serde_json::json!({
        "digest": s.digest.as_ref().map(|d| d.as_str()),
        "size": s.size,
    })
}

/// The JSON shape of one delta entry, one object per line.
fn delta_json(d: &nexus_raw_core::Delta) -> serde_json::Value {
    match d {
        nexus_raw_core::Delta::Same { name, digest } => {
            serde_json::json!({"delta": "same", "name": name.to_string(), "digest": digest.as_str()})
        }
        nexus_raw_core::Delta::MissingLocal { name, remote } => {
            serde_json::json!({"delta": "missing-local", "name": name.to_string(), "remote": delta_side_json(remote)})
        }
        nexus_raw_core::Delta::MissingRemote { name, local } => {
            serde_json::json!({"delta": "missing-remote", "name": name.to_string(), "local": delta_side_json(local)})
        }
        nexus_raw_core::Delta::Diverged {
            name,
            local,
            remote,
            size,
            sha,
        } => serde_json::json!({
            "delta": "diverged",
            "name": name.to_string(),
            "local": delta_side_json(local),
            "remote": delta_side_json(remote),
            "size": size,
            "sha": sha,
        }),
    }
}

/// Delete the enumerated names from a remote directory: marker first, then bytes.
pub(crate) async fn rm(
    cli: &Cli,
    src: &str,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let result = run_rm(&ctx, manifest, names, ls, dry_run, prefixes).await;
    finish(ctx).await;
    result
}

async fn run_rm(
    ctx: &Ctx,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let enum_src = enumeration_source(ctx, manifest, names, ls)
        .await?
        .with_prefixes(prefixes)?;
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
    args: MirrorArgs<'_>,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    // Both facades build before anything moves: a bad destination URL is misuse, not a half-poured version.
    // Per-side credentials override the shared `-u`: a mirror between different servers
    // must not leak one server's secret to the other.
    let src_nxr = make_nxr_with(cli, args.src, session.sender(), args.src_user)?;
    let dst_nxr = make_nxr_with(cli, args.dst, session.sender(), args.dst_user)?;
    let enum_src = source_enumeration(&src_nxr, manifest, names, ls)
        .await?
        .with_prefixes(prefixes)?;
    let result = run_mirror(&src_nxr, &dst_nxr, enum_src, dry_run, cli.json).await;
    // Both facades must die before the renderer drains: each holds a sender clone.
    drop(src_nxr);
    drop(dst_nxr);
    session.finish().await;
    result
}

/// Move: mirror the enumerated names into the destination, then delete them at the source.
///
/// Nothing is deleted until the pour converged: a failed mirror leaves the source untouched,
/// and a failed delete after a converged pour leaves a duplicate, never a loss.
pub(crate) async fn mv(
    cli: &Cli,
    args: MirrorArgs<'_>,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
    dry_run: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let src_nxr = make_nxr_with(cli, args.src, session.sender(), args.src_user)?;
    let dst_nxr = make_nxr_with(cli, args.dst, session.sender(), args.dst_user)?;
    let enum_src = source_enumeration(&src_nxr, manifest, names, ls)
        .await?
        .with_prefixes(prefixes)?;
    let result = run_mv(&src_nxr, &dst_nxr, enum_src, dry_run, cli.json).await;
    drop(src_nxr);
    drop(dst_nxr);
    session.finish().await;
    result
}

/// The enumeration of a source repository: `--ls`, `--manifest`, repeatable `--name`, or the conventional `manifest.json` at the source directory URL.
async fn source_enumeration(
    src: &Nxr,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
) -> Result<Enumeration, Error> {
    if ls {
        return Ok(Enumeration::Search);
    }
    if let Some(spec) = manifest {
        return Ok(Enumeration::Manifest(load_manifest(src, spec).await?));
    }
    if !names.is_empty() {
        return Ok(Enumeration::Names(parse_names(names)?));
    }
    match src.manifest_at_base().await? {
        Some(m) => Ok(Enumeration::Manifest(m)),
        None => Err(Error::Enumerate {
            url: src.base().to_owned(),
            reason: "no manifest.json at the source and no --manifest/--name/--ls given".into(),
        }),
    }
}

async fn run_mv(
    src: &Nxr,
    dst: &Nxr,
    enum_src: Enumeration,
    dry_run: bool,
    json: bool,
) -> Result<(), Error> {
    // One resolution feeds both phases: the delete removes exactly what the pour poured.
    let names = src.enumerate(enum_src).await?;
    if dry_run {
        println!("will move:");
        for a in src
            .mirror_plan(dst, Enumeration::Names(names.clone()))
            .await?
        {
            let (line, json_shape) = mirror_plan_line(&a);
            crate::cmd::print_line(json, &line, &json_shape);
        }
        println!("will delete:");
        for a in src.rm_plan(Enumeration::Names(names)).await? {
            crate::cmd::print_line(json, &rm_plan_line(&a), &rm_action_json(&a));
        }
        return Ok(());
    }
    // Phase one: the pour, exactly like `mirror`.
    src.mirror(dst, Enumeration::Names(names.clone())).await?;
    // Phase two: the delete, exactly like `rm` (marker first, bytes follow).
    // A refusal here is a duplicate across repositories, never a loss: the destination holds every name.
    if !json {
        eprintln!("delete phase:");
    }
    src.rm(Enumeration::Names(names)).await?;
    Ok(())
}

async fn run_mirror(
    src: &Nxr,
    dst: &Nxr,
    enum_src: Enumeration,
    dry_run: bool,
    json: bool,
) -> Result<(), Error> {
    if dry_run {
        for a in src.mirror_plan(dst, enum_src).await? {
            let (line, json_shape) = mirror_plan_line(&a);
            crate::cmd::print_line(json, &line, &json_shape);
        }
        return Ok(());
    }
    src.mirror(dst, enum_src).await?;
    Ok(())
}

/// The plan line of a mirror action, human and JSON shapes.
fn mirror_plan_line(a: &nexus_raw_core::MirrorAction) -> (String, serde_json::Value) {
    match a {
        nexus_raw_core::MirrorAction::Skip { name, .. } => (
            format!("skip {name}"),
            serde_json::json!({"plan": "skip", "name": name.as_str()}),
        ),
        nexus_raw_core::MirrorAction::Copy { name, size, .. } => (
            format!("copy {name}"),
            serde_json::json!({"plan": "copy", "name": name.as_str(), "size": size}),
        ),
    }
}
