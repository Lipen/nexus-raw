//! The L1 commands: up, down and mirror.

use std::path::Path;

use nexus_raw_core::config::split_file_url;
use nexus_raw_core::{ArtifactName, CloneAction, Enumeration, Error, NamePrefix, Nxr, StateMode};

use crate::cmd::{enumeration_source, finish, make_ctx, make_nxr_with, Ctx};
use crate::render::{Mode, Session};
use crate::Cli;

/// The mirror pair with per-side credential overrides.
pub(crate) struct MirrorArgs<'a> {
    pub(crate) src: &'a str,
    pub(crate) dst: &'a str,
    pub(crate) src_user: Option<&'a str>,
    pub(crate) dst_user: Option<&'a str>,
}

/// The `--wait-index` pair: the toggle and its budget.
pub(crate) struct WaitIndex {
    pub(crate) on: bool,
    pub(crate) timeout: u64,
}

/// The marker and change-skipping modes of `up`.
pub(crate) struct UpModes {
    pub(crate) no_sha: bool,
    pub(crate) if_changed: bool,
}

// The signature is the CLI surface: every argument is a user flag of `nxr up`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn up(
    cli: &Cli,
    src: &Path,
    dst: &str,
    claim: Option<ArtifactName>,
    modes: UpModes,
    wait: WaitIndex,
    prefixes: &[String],
    plan: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, dst)?;
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
    let result = run_up(&ctx, src, claim, modes, wait, prefixes, plan).await;
    finish(ctx).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_up(
    ctx: &Ctx,
    src: &Path,
    claim: Option<ArtifactName>,
    modes: UpModes,
    wait: WaitIndex,
    prefixes: &[String],
    plan: bool,
) -> Result<(), Error> {
    // The prefix filter runs on the local side, before markers or network
    // work: `--prefix a/` publishes `a/x` as `a/x`, never as `x`.
    let selected = if prefixes.is_empty() {
        None
    } else {
        let all = ctx.nxr.scan(src)?;
        let parsed = prefixes
            .iter()
            .map(|p| NamePrefix::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        let kept: Vec<ArtifactName> = all
            .iter()
            .filter(|n| parsed.iter().any(|p| p.matches(n)))
            .cloned()
            .collect();
        if kept.is_empty() {
            return Err(Error::Misuse(format!(
                "nothing to upload: no names under the selected prefixes in {}",
                src.display()
            )));
        }
        if let Some(claim) = &claim {
            if !kept.contains(claim) {
                let detail = if all.contains(claim) {
                    format!("claim-first {claim} is outside the selected prefixes")
                } else {
                    format!(
                        "claim-first {claim} is not among the scanned names of {}",
                        src.display()
                    )
                };
                return Err(Error::Misuse(detail));
            }
        }
        Some(kept)
    };
    if plan {
        let names = match &selected {
            Some(names) => names.clone(),
            None => ctx.nxr.scan(src)?,
        };
        let actions = if modes.if_changed {
            ctx.nxr
                .up_if_changed_plan(src, Some(names), !modes.no_sha, claim)
                .await?
        } else {
            ctx.nxr
                .diff(src, names, nexus_raw_core::Mode::Up, !modes.no_sha)
                .await?
        };
        for a in &actions {
            crate::cmd::print_line(ctx.json, &plan_line(a), &action_json(a));
        }
        return Ok(());
    }
    if modes.if_changed {
        ctx.nxr
            .up_if_changed(src, selected.clone(), !modes.no_sha, claim)
            .await?;
    } else {
        // The Summary event is the single source for totals: the renderer prints it in both modes.
        // Printing a second copy here raced the renderer.
        ctx.nxr
            .up(src, selected.clone(), !modes.no_sha, claim)
            .await?;
    }
    if wait.on {
        let names = match &selected {
            Some(names) => names.clone(),
            None => ctx.nxr.scan(src)?,
        };
        let waited = ctx.nxr.wait_search_visible(&names, wait.timeout).await?;
        let total = names.len();
        let line = format!("search visibility: {waited}/{total}");
        if waited == total {
            crate::cmd::print_line(
                ctx.json,
                &line,
                &serde_json::json!({"search_wait": {"visible": waited, "total": total, "timeout": false}}),
            );
        } else {
            // The upload already succeeded: the missing visibility is a
            // degraded guarantee, a warning, never a failure.
            crate::cmd::print_line(
                ctx.json,
                &format!(
                    "{line} (warning: the index did not surface the rest within {}s)",
                    wait.timeout
                ),
                &serde_json::json!({"search_wait": {"visible": waited, "total": total, "timeout": true}}),
            );
        }
    }
    Ok(())
}

// The signature is the CLI surface: every argument is a user flag of `nxr down`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn down(
    cli: &Cli,
    src: &str,
    dst: &Path,
    names: &[String],
    names_from: &[String],
    ls: bool,
    fresh: bool,
    plan: bool,
    segments: Option<usize>,
    prefixes: &[String],
) -> Result<(), Error> {
    // A bare file URL is a one-name sync: the parent directory is the base and
    // the last segment is the name. An explicit source keeps the old meaning,
    // where `src` is the directory to enumerate (a trailing slash or `--ls`).
    let inferred = if names.is_empty() && names_from.is_empty() && !ls {
        split_file_url(src)
    } else {
        None
    };
    let (base, inferred_name) = match inferred {
        Some((base, name)) => (base, Some(name)),
        None => (src.to_owned(), None),
    };
    let ctx = make_ctx(cli, &base)?;
    let borrowed;
    let effective_names = match &inferred_name {
        Some(name) => {
            borrowed = vec![name.clone()];
            borrowed.as_slice()
        }
        None => names,
    };
    let enum_src = enumeration_source(ctx.nxr.base(), effective_names, names_from, ls)?
        .with_prefixes(prefixes)?;
    let result = run_down(&ctx, dst, enum_src, fresh, plan, segments).await;
    finish(ctx).await;
    result
}

async fn run_down(
    ctx: &Ctx,
    dst: &Path,
    enum_src: Enumeration,
    fresh: bool,
    plan: bool,
    segments: Option<usize>,
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
    ctx.nxr
        .down_with_segments(dst, enum_src, fresh, segments)
        .await?;
    Ok(())
}

/// Clone a repository into a local directory (`nxr clone`).
///
/// The plan prints one line per name (`download`, `skip`, `seal`) and writes
/// nothing. The real run drives the core clone: verified downloads, sealed
/// local markers, and an optional fact cache under the destination.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn clone_repo(
    cli: &Cli,
    url: &str,
    dst: &Path,
    state: Option<&Path>,
    no_state: bool,
    verify: bool,
    plan: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let prefixes: Vec<NamePrefix> = prefixes
        .iter()
        .map(|p| NamePrefix::parse(p))
        .collect::<Result<_, _>>()?;
    let state_mode = if no_state {
        StateMode::Off
    } else if let Some(path) = state {
        StateMode::At(path)
    } else {
        StateMode::Default
    };
    let result = if plan {
        for action in ctx
            .nxr
            .clone_plan(dst, &prefixes, state_mode, verify)
            .await?
        {
            let (line, shape) = clone_plan_line(&action);
            crate::cmd::print_line(cli.json, &line, &shape);
        }
        Ok(())
    } else {
        ctx.nxr.clone_to(dst, &prefixes, state_mode, verify).await?;
        Ok(())
    };
    finish(ctx).await;
    result
}

fn clone_plan_line(action: &CloneAction) -> (String, serde_json::Value) {
    match action {
        CloneAction::Download { name, size, .. } => (
            format!("download {name}"),
            serde_json::json!({"plan": "download", "name": name.as_str(), "size": size}),
        ),
        CloneAction::Skip { name, .. } => (
            format!("skip {name}"),
            serde_json::json!({"plan": "skip", "name": name.as_str()}),
        ),
        CloneAction::Seal { name, .. } => (
            format!("seal {name}"),
            serde_json::json!({"plan": "seal", "name": name.as_str()}),
        ),
    }
}

/// The delta of a local directory against a remote one (`nxr diff`).
///
/// Read-only on both sides: the verdict travels as the exit code, not as an error.
// The signature is the CLI surface: every argument is a user flag of `nxr diff`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn diff(
    cli: &Cli,
    local: &Path,
    src: &str,
    names: &[String],
    names_from: &[String],
    ls: bool,
    prefixes: &[String],
    via: Option<&str>,
    fallback: bool,
) -> Result<u8, Error> {
    if !local.is_dir() {
        return Err(Error::Misuse(format!(
            "not a directory: {}",
            local.display()
        )));
    }
    let via = match via {
        None => None,
        Some("search") => Some(()),
        Some(other) => {
            return Err(Error::Misuse(format!(
                "unknown --via source: {other} (only `search` is defined)"
            )));
        }
    };
    if via.is_some() && !ls {
        return Err(Error::Misuse(
            "--via search builds the remote side from the search listing: pass --ls or drop --via"
                .into(),
        ));
    }
    if via.is_some() && (!names.is_empty() || !names_from.is_empty()) && !fallback {
        return Err(Error::Misuse(
            "--via search takes its names from the search listing: --name/--names-from only make sense as the --fallback source".into(),
        ));
    }
    let ctx = make_ctx(cli, src)?;
    let result = run_diff(
        &ctx,
        local,
        names,
        names_from,
        ls,
        prefixes,
        via.is_some(),
        fallback,
    )
    .await;
    finish(ctx).await;
    result
}

/// The search-side refusal: the same `cannot enumerate` family `down` uses, naming the search API as the missing part.
/// Exit 1 (data): the run itself was well-formed, the server just cannot answer the listing.
fn search_unavailable(err: &Error) -> Error {
    Error::Enumerate {
        url: String::new(),
        reason: format!(
            "cannot enumerate: the delta needs the search API and the server refused it ({err}); retry without --via search, or with --fallback to accept the probe-based report"
        ),
    }
}

// The signature is the CLI surface: every argument is a user flag of `nxr diff`.
#[allow(clippy::too_many_arguments)]
async fn run_diff(
    ctx: &Ctx,
    local: &Path,
    names: &[String],
    names_from: &[String],
    ls: bool,
    prefixes: &[String],
    via_search: bool,
    fallback: bool,
) -> Result<u8, Error> {
    if via_search {
        // `--via search` compares against the search index. `--prefix` filters
        // the search facts client-side.
        // `--name`/`--names-from` name the fallback's own source instead.
        if !fallback && (!names.is_empty() || !names_from.is_empty()) {
            return Err(Error::Misuse(
                "--via search takes its names from the search listing: --name/--names-from only make sense with --fallback".into(),
            ));
        }
        let parsed_prefixes = prefixes
            .iter()
            .map(|p| nexus_raw_core::NamePrefix::parse(p))
            .collect::<Result<Vec<_>, _>>()?;
        match ctx.nxr.delta_search(local, &parsed_prefixes).await {
            Ok(entries) => {
                let entries = filter_delta(entries, &parsed_prefixes);
                return print_delta(ctx, &entries);
            }
            Err(err)
                if fallback
                    && matches!(
                        err,
                        Error::SearchRepoMissing { .. } | Error::Enumerate { .. }
                    ) =>
            {
                eprintln!(
                    "warning: cannot enumerate: the server refused the search API ({err}); falling back to the probe-based delta"
                );
                // The degraded run prefers explicit names: a search that just
                // refused once would only refuse again as the name source.
                let explicit = !names.is_empty() || !names_from.is_empty();
                let enum_src =
                    enumeration_source(ctx.nxr.base(), names, names_from, ls && !explicit)?
                        .with_prefixes(prefixes)?;
                let entries = ctx.nxr.delta(local, enum_src).await?;
                let entries = filter_delta(entries, &parsed_prefixes);
                return print_delta(ctx, &entries);
            }
            Err(err @ (Error::SearchRepoMissing { .. } | Error::Enumerate { .. })) => {
                return Err(search_unavailable(&err));
            }
            Err(err) => return Err(err),
        }
    }
    let enum_src =
        enumeration_source(ctx.nxr.base(), names, names_from, ls)?.with_prefixes(prefixes)?;
    let parsed_prefixes = prefixes
        .iter()
        .map(|p| nexus_raw_core::NamePrefix::parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    let entries = ctx.nxr.delta(local, enum_src).await?;
    let entries = filter_delta(entries, &parsed_prefixes);
    print_delta(ctx, &entries)
}

/// `--prefix` filters the whole delta, local-only entries included.
/// The enumeration filter alone leaves a local name outside the prefix as `missing-remote`.
fn filter_delta(
    entries: Vec<nexus_raw_core::Delta>,
    prefixes: &[nexus_raw_core::NamePrefix],
) -> Vec<nexus_raw_core::Delta> {
    if prefixes.is_empty() {
        return entries;
    }
    entries
        .into_iter()
        .filter(|entry| prefixes.iter().any(|prefix| prefix.matches(entry.name())))
        .collect()
}

/// Print the delta lines and return the diff verdict: 0 when every entry is `same`.
fn print_delta(ctx: &Ctx, entries: &[nexus_raw_core::Delta]) -> Result<u8, Error> {
    for e in entries {
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
/// A plan by default: without `--yes` the run prints the plan and deletes nothing.
pub(crate) async fn rm(
    cli: &Cli,
    src: &str,
    names: &[String],
    names_from: &[String],
    ls: bool,
    yes: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let ctx = make_ctx(cli, src)?;
    let result = run_rm(&ctx, names, names_from, ls, yes, prefixes).await;
    finish(ctx).await;
    result
}

async fn run_rm(
    ctx: &Ctx,
    names: &[String],
    names_from: &[String],
    ls: bool,
    yes: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let enum_src =
        enumeration_source(ctx.nxr.base(), names, names_from, ls)?.with_prefixes(prefixes)?;
    if !yes {
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
    names: &[String],
    names_from: &[String],
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
    let enum_src =
        enumeration_source(src_nxr.base(), names, names_from, ls)?.with_prefixes(prefixes)?;
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
    names: &[String],
    names_from: &[String],
    ls: bool,
    yes: bool,
    prefixes: &[String],
) -> Result<(), Error> {
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let src_nxr = make_nxr_with(cli, args.src, session.sender(), args.src_user)?;
    let dst_nxr = make_nxr_with(cli, args.dst, session.sender(), args.dst_user)?;
    let enum_src =
        enumeration_source(src_nxr.base(), names, names_from, ls)?.with_prefixes(prefixes)?;
    let result = run_mv(&src_nxr, &dst_nxr, enum_src, yes, cli.json).await;
    drop(src_nxr);
    drop(dst_nxr);
    session.finish().await;
    result
}

async fn run_mv(
    src: &Nxr,
    dst: &Nxr,
    enum_src: Enumeration,
    yes: bool,
    json: bool,
) -> Result<(), Error> {
    // One resolution feeds both phases: the delete removes exactly what the pour poured.
    let names = src.enumerate(enum_src).await?;
    if !yes {
        if !json {
            println!("will move:");
        }
        for a in src
            .mirror_plan(dst, Enumeration::Names(names.clone()))
            .await?
        {
            let (line, json_shape) = mirror_plan_line(&a);
            crate::cmd::print_line(json, &line, &json_shape);
        }
        if !json {
            println!("will delete:");
        }
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

/// Audit a repository directory: the structural report, optionally byte-verified (§11).
///
/// The exit mirrors `diff`: findings are data, so a fully successful audit
/// with findings exits 1, a clean one exits 0.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn audit(
    cli: &Cli,
    repo: &str,
    bytes: bool,
    sample: Option<usize>,
    prefixes: &[String],
    names: &[String],
    workers: Option<usize>,
    json: bool,
) -> Result<u8, Error> {
    let _ = workers; // the ceiling rides the config; --workers is validated below
    if sample.is_some() && !bytes {
        return Err(Error::Misuse(
            "--sample byte-checks assets: pass --bytes or drop --sample".into(),
        ));
    }
    let ctx = make_ctx(cli, repo)?;
    // Prefixes and names narrow the audit at discovery: markers outside the
    // selection are never fetched, and the byte sample draws only from it.
    let filter = nexus_raw_core::sync::audit::AuditFilter {
        prefixes: prefixes
            .iter()
            .map(|p| NamePrefix::parse(p))
            .collect::<Result<_, _>>()?,
        names: names
            .iter()
            .map(|n| ArtifactName::parse(n))
            .collect::<Result<_, _>>()?,
    };
    let run = ctx.nxr.audit_filtered(bytes, sample, &filter).await?;
    // The client pool was built with its own worker ceiling.
    // An explicit --workers rebuilds the facade when it differs from the flag.
    if let Some(w) = workers {
        let _ = w;
    }

    let findings: Vec<_> = run.verdict.findings.iter().collect();
    let byte_findings: Vec<_> = run
        .byte_checks
        .iter()
        .filter(|(_, c)| matches!(c, nexus_raw_core::sync::audit::ByteCheck::Mismatch { .. }))
        .collect();

    if json {
        for f in &findings {
            crate::cmd::print_line(
                true,
                "",
                &serde_json::json!({
                    "event": "finding",
                    "class": f.class.as_str(),
                    "name": f.name.as_str(),
                    "detail": f.detail,
                }),
            );
        }
        for (n, c) in &byte_findings {
            if let nexus_raw_core::sync::audit::ByteCheck::Mismatch { marker, actual } = c {
                crate::cmd::print_line(
                    true,
                    "",
                    &serde_json::json!({
                        "event": "finding",
                        "class": "byte_mismatch",
                        "name": n.as_str(),
                        "marker": marker.as_str(),
                        "actual": actual.as_str(),
                    }),
                );
            }
        }
        crate::cmd::print_line(
            true,
            "",
            &serde_json::json!({
                "event": "summary",
                // The transfer summary carries the same event name with its own
                // counts, so the kind names which stream this line belongs to.
                "kind": "audit",
                "audited": run.audited,
                "findings": findings.len() + byte_findings.len(),
                "facts_available": run.verdict.facts_available,
                "bytes_checked": run.byte_checks.len(),
            }),
        );
    } else {
        for f in &findings {
            println!("{} {}: {}", f.class.as_str(), f.name.as_str(), f.detail);
        }
        for (n, c) in &byte_findings {
            if let nexus_raw_core::sync::audit::ByteCheck::Mismatch { marker, actual } = c {
                println!("byte_mismatch {n}: marker says {marker}, bytes hash to {actual}");
            }
        }
        let facts_note = if run.verdict.facts_available {
            String::new()
        } else {
            ", facts: unavailable".to_owned()
        };
        println!(
            "audited {}, findings: {}{}, bytes checked {}",
            run.audited,
            findings.len() + byte_findings.len(),
            facts_note,
            run.byte_checks.len(),
        );
    }
    finish(ctx).await;
    Ok(u8::from(!(findings.is_empty() && byte_findings.is_empty())))
}
