//! Command handlers and the helpers they share.

mod doctor;
mod layout;
mod primitives;
mod transfer;

use std::path::Path;
use std::time::Duration;

use nexus_raw_core::{creds, ArtifactName, Config, Enumeration, Error, Event, Manifest, Nxr};

use crate::render::{Mode, Session};
use crate::{ChannelOp, Cli, Cmd};

pub(crate) async fn dispatch(cli: &Cli) -> Result<(), Error> {
    match &cli.command {
        Cmd::Get { url, out, cont } => primitives::get(cli, url, out.as_deref(), *cont).await,
        Cmd::Put { url, file, sha } => primitives::put(cli, url, file, *sha).await,
        Cmd::Head { url } => primitives::head(cli, url).await,
        Cmd::Sha { target } => primitives::sha(cli, target).await,
        Cmd::Up {
            src,
            dst,
            manifest,
            claim_first,
            no_sha,
            dry_run,
        } => {
            let claim = match claim_first {
                Some(raw) => Some(ArtifactName::parse(raw)?),
                None => None,
            };
            transfer::up(cli, src, dst, manifest.as_deref(), claim, *no_sha, *dry_run).await
        }
        Cmd::Down {
            src,
            dst,
            manifest,
            name,
            ls,
            fresh,
            dry_run,
        } => {
            transfer::down(
                cli,
                src,
                dst,
                manifest.as_deref(),
                name,
                *ls,
                *fresh,
                *dry_run,
            )
            .await
        }
        Cmd::Rm {
            src,
            manifest,
            name,
            ls,
            dry_run,
        } => transfer::rm(cli, src, manifest.as_deref(), name, *ls, *dry_run).await,
        Cmd::Point { clear, url } => {
            if !clear {
                return Err(Error::Misuse(
                    "point needs --clear: deleting the pointer is the only operation it has"
                        .to_owned(),
                ));
            }
            layout::point_clear(cli, url).await
        }
        Cmd::Mirror {
            src,
            dst,
            manifest,
            name,
            ls,
            dry_run,
            src_user,
            dst_user,
        } => {
            transfer::mirror(
                cli,
                transfer::MirrorArgs {
                    src,
                    dst,
                    src_user: src_user.as_deref(),
                    dst_user: dst_user.as_deref(),
                },
                manifest.as_deref(),
                name,
                *ls,
                *dry_run,
            )
            .await
        }
        Cmd::Ls { url, assets } => layout::ls(cli, url, *assets).await,
        Cmd::Channel { op } => match op {
            ChannelOp::Get { url } => layout::channel_get(cli, url).await,
            ChannelOp::Set {
                url,
                token,
                if_forward,
            } => layout::channel_set(cli, url, token, *if_forward).await,
        },
        Cmd::Verify { dir, manifest } => layout::verify(cli, dir, manifest.as_deref()).await,
        Cmd::Doctor { url } => doctor::run(cli, url.as_deref()).await,
    }
}

/// A running invocation: the facade plus the event renderer.
pub(crate) struct Ctx {
    pub(crate) nxr: Nxr,
    pub(crate) session: Session,
    pub(crate) json: bool,
}

/// Build the facade for `base`: creds from `-u` or env, renderer attached.
pub(crate) fn make_ctx(cli: &Cli, base: &str) -> Result<Ctx, Error> {
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let nxr = make_nxr(cli, base, session.sender())?;
    Ok(Ctx {
        nxr,
        session,
        json: cli.json,
    })
}

/// Build the facade for `base` against an existing event sender.
/// Mirror builds two of these (source and destination) on one session.
pub(crate) fn make_nxr(
    cli: &Cli,
    base: &str,
    sender: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<Nxr, Error> {
    make_nxr_with(cli, base, sender, cli.user.as_deref())
}

/// The same, with an explicit credentials override (`mirror --src-user/--dst-user`):
/// the override replaces `-u` for that one side, the env fallback stays last.
pub(crate) fn make_nxr_with(
    cli: &Cli,
    base: &str,
    sender: tokio::sync::mpsc::UnboundedSender<Event>,
    explicit_user: Option<&str>,
) -> Result<Nxr, Error> {
    let auth = match split_user(explicit_user)? {
        Some((u, p)) => creds::resolve(Some((u, p)))?.map(|c| c.header),
        None => creds::resolve(None)?.map(|c| c.header),
    };
    let cfg = Config {
        base: base.to_owned(),
        tls_insecure: cli.tls_insecure,
        workers: cli.workers,
        retry_attempts: cli.retry,
        connect_timeout: Duration::from_secs(cli.connect_timeout_secs),
        stall_timeout: Duration::from_secs(cli.stall_secs),
        auth,
    };
    Nxr::new(cfg, sender)
}

/// Close the event channel and wait for the renderer to drain.
///
/// The facade must die first: it holds a sender clone through the client's `Progress`, and the renderer only finishes when the channel closes.
pub(crate) async fn finish(ctx: Ctx) {
    let Ctx {
        nxr,
        session,
        json: _,
    } = ctx;
    drop(nxr);
    session.finish().await;
}

/// `user:pass` split on the first `:`.
fn split_user(user: Option<&str>) -> Result<Option<(&str, &str)>, Error> {
    let Some(u) = user else { return Ok(None) };
    let Some((user, pass)) = u.split_once(':') else {
        return Err(Error::Misuse(
            "-u expects user:pass (a single ':'); the value carries none".to_owned(),
        ));
    };
    Ok(Some((user, pass)))
}

/// Parse explicit `--name` values through the grammar (exit 2 on bad names).
pub(crate) fn parse_names(names: &[String]) -> Result<Vec<ArtifactName>, Error> {
    names.iter().map(|s| ArtifactName::parse(s)).collect()
}

/// Resolve the enumeration source of `down` and `rm`: `--ls`, `--manifest`, repeatable `--name`, or the conventional `manifest.json` at the directory URL.
/// Without any source the call refuses (exit 1) with the enumeration hint.
pub(crate) async fn enumeration_source(
    ctx: &Ctx,
    manifest: Option<&str>,
    names: &[String],
    ls: bool,
) -> Result<Enumeration, Error> {
    if ls {
        return Ok(Enumeration::Search);
    }
    if let Some(spec) = manifest {
        return Ok(Enumeration::Manifest(load_manifest(&ctx.nxr, spec).await?));
    }
    if !names.is_empty() {
        return Ok(Enumeration::Names(parse_names(names)?));
    }
    // The convention: a manifest.json in the version directory.
    match ctx.nxr.manifest_at_base().await? {
        Some(m) => Ok(Enumeration::Manifest(m)),
        None => Err(Error::Enumerate {
            url: ctx.nxr.base().to_owned(),
            reason: "no manifest.json on the server and no --manifest/--name/--ls given".into(),
        }),
    }
}

/// Resolve a `--manifest` spec: `-` for stdin, http(s) URLs through the server, everything else as a local file.
pub(crate) async fn load_manifest(nxr: &Nxr, spec: &str) -> Result<Manifest, Error> {
    match spec {
        "-" => Manifest::from_stdin(),
        u if u.starts_with("http://") || u.starts_with("https://") => nxr.manifest_from(u).await,
        p => Manifest::from_file(Path::new(p)),
    }
}

/// One JSON line for the simple (non-event) results.
pub(crate) fn print_line(json: bool, human: &str, value: &serde_json::Value) {
    if json {
        println!("{value}");
    } else {
        println!("{human}");
    }
}
