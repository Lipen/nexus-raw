//! Command handlers and the helpers they share.

mod doctor;
mod layout;
mod primitives;
mod transfer;

use std::path::Path;
use std::time::Duration;

use nexus_raw_core::{creds, ArtifactName, Config, Error, Manifest, Nxr};

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
        } => transfer::down(cli, src, dst, manifest.as_deref(), name, *ls, *fresh).await,
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
    let auth = match split_user(cli.user.as_deref())? {
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
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let nxr = Nxr::new(cfg, session.sender())?;
    Ok(Ctx {
        nxr,
        session,
        json: cli.json,
    })
}

/// Close the event channel and wait for the renderer to drain.
///
/// The facade must die first: it holds a sender clone through the client's
/// `Progress`, and the renderer only finishes when the channel closes.
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

/// Resolve a `--manifest` spec: `-` for stdin, http(s) URLs through the
/// server, everything else as a local file.
pub(crate) async fn load_manifest(nxr: &Nxr, spec: &str) -> Result<Manifest, Error> {
    match spec {
        "-" => Manifest::from_stdin(),
        u if u.starts_with("http://") || u.starts_with("https://") => nxr.manifest_from(u).await,
        p => Manifest::from_file(Path::new(p)),
    }
}

/// One JSON line for the simple (non-event) results.
pub(crate) fn print_line(json: bool, human: String, value: serde_json::Value) {
    if json {
        println!("{value}");
    } else {
        println!("{human}");
    }
}
