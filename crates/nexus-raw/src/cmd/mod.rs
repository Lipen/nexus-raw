//! Command handlers and the helpers they share.

mod complete;
mod doctor;
mod layout;
mod primitives;
mod transfer;

use std::time::Duration;

use nexus_raw_core::{creds, ArtifactName, Config, Enumeration, Error, Event, Nxr};

use crate::render::{Mode, Session};
use crate::{ChannelOp, Cli, Cmd, ServiceOp};

/// Route one invocation to its handler.
///
/// The success value is the process exit code: `diff` compares and may exit 1
/// after a fully successful run, every other command exits 0 on success.
pub(crate) async fn dispatch(cli: &Cli) -> Result<u8, Error> {
    if let Cmd::Diff {
        local,
        src,
        name,
        names_from,
        ls,
        prefix,
        via,
        fallback,
    } = &cli.command
    {
        return transfer::diff(
            cli,
            local,
            src,
            name,
            names_from,
            *ls,
            prefix,
            via.as_deref(),
            *fallback,
        )
        .await;
    }
    if let Cmd::Audit {
        repo,
        bytes,
        sample,
        prefix,
        name,
        workers,
        json,
    } = &cli.command
    {
        return transfer::audit(cli, repo, *bytes, *sample, prefix, name, *workers, *json).await;
    }
    run(cli).await.map(|()| 0)
}

async fn run(cli: &Cli) -> Result<(), Error> {
    match &cli.command {
        // Routed in `dispatch` (it owns the exit-1 verdict), like `diff`.
        Cmd::Audit { .. } => unreachable!("audit is routed in dispatch"),
        Cmd::Get {
            url,
            out,
            cont,
            segments,
        } => primitives::get(cli, url, out.as_deref(), *cont, *segments).await,
        Cmd::Put {
            url,
            file,
            stdin,
            sha,
        } => primitives::put(cli, url, file.as_deref(), *stdin, *sha).await,
        Cmd::Head { url } => primitives::head(cli, url).await,
        Cmd::Sha { target } => primitives::sha(cli, target).await,
        Cmd::Up {
            src,
            dst,
            claim_first,
            no_sha,
            if_changed,
            wait_index,
            index_timeout,
            plan,
        } => {
            let claim = match claim_first {
                Some(raw) => Some(ArtifactName::parse(raw)?),
                None => None,
            };
            transfer::up(
                cli,
                src,
                dst,
                claim,
                transfer::UpModes {
                    no_sha: *no_sha,
                    if_changed: *if_changed,
                },
                transfer::WaitIndex {
                    on: *wait_index,
                    timeout: *index_timeout,
                },
                *plan,
            )
            .await
        }
        Cmd::Down {
            src,
            dst,
            name,
            names_from,
            ls,
            fresh,
            segments,
            plan,
            prefix,
        } => {
            transfer::down(
                cli, src, dst, name, names_from, *ls, *fresh, *plan, *segments, prefix,
            )
            .await
        }
        Cmd::Clone {
            url,
            dst,
            state,
            no_state,
            verify,
            plan,
            prefix,
        } => {
            transfer::clone_repo(
                cli,
                url,
                dst,
                state.as_deref(),
                *no_state,
                *verify,
                *plan,
                prefix,
            )
            .await
        }
        Cmd::Rm {
            src,
            name,
            names_from,
            ls,
            yes,
            prefix,
        } => transfer::rm(cli, src, name, names_from, *ls, *yes, prefix).await,
        Cmd::Point { url } => layout::point_clear(cli, url).await,
        Cmd::Mirror {
            src,
            dst,
            name,
            names_from,
            ls,
            dry_run,
            src_user,
            dst_user,
            prefix,
        } => {
            transfer::mirror(
                cli,
                transfer::MirrorArgs {
                    src,
                    dst,
                    src_user: src_user.as_deref(),
                    dst_user: dst_user.as_deref(),
                },
                name,
                names_from,
                *ls,
                *dry_run,
                prefix,
            )
            .await
        }
        Cmd::Mv {
            src,
            dst,
            name,
            names_from,
            ls,
            yes,
            src_user,
            dst_user,
            prefix,
        } => {
            transfer::mv(
                cli,
                transfer::MirrorArgs {
                    src,
                    dst,
                    src_user: src_user.as_deref(),
                    dst_user: dst_user.as_deref(),
                },
                name,
                names_from,
                *ls,
                *yes,
                prefix,
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
        Cmd::Verify { dir } => layout::verify(cli, dir).await,
        Cmd::Service { op } => match op {
            ServiceOp::Repos { url } => layout::service_repos(cli, url).await,
            ServiceOp::Status { url } => layout::service_status(cli, url).await,
            ServiceOp::Repo { url, detail } => layout::service_repo(cli, url, *detail).await,
            ServiceOp::Assets { url, q, prefix } => {
                layout::service_assets(cli, url, q.as_deref(), prefix).await
            }
            ServiceOp::Eula { url, accept } => layout::service_eula(cli, url, *accept).await,
        },
        Cmd::Doctor {
            url,
            deep,
            probe_name,
        } => doctor::run(cli, url.as_deref(), *deep, probe_name.as_deref()).await,
        Cmd::Complete { shell } => complete::complete(shell),
        // Routed in `dispatch`, where the verdict becomes the exit code.
        Cmd::Diff { .. } => Ok(()),
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
    // Priority: -u (or the mirror's per-side override) over the alias pair
    // over the ambient env. The alias pair is resolved in main before dispatch.
    // A transfer with per-side overrides (`--src-user`/`--dst-user`) still falls
    // back to the shared `-u`, then the alias, then the environment.
    let explicit = explicit_user.or(cli.user.as_deref());
    let auth = match split_user(explicit)? {
        Some((u, p)) => creds::resolve(Some((u, p)))?.map(|c| c.header),
        None => match cli.alias_creds.as_ref() {
            Some((u, p)) => creds::resolve(Some((u, p)))?.map(|c| c.header),
            None => creds::resolve(None)?.map(|c| c.header),
        },
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
use std::io::Read;

pub(crate) fn parse_names(names: &[String]) -> Result<Vec<ArtifactName>, Error> {
    names.iter().map(|s| ArtifactName::parse(s)).collect()
}

/// Read one `--names-from` source: `-` is stdin, one name per line.
/// Lines are trimmed; empty lines are skipped; anything else must parse
/// through the same name grammar as `--name`.
pub(crate) fn read_names_file(path: &str) -> Result<Vec<String>, Error> {
    let text = if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| Error::Misuse(format!("cannot read --names-from -: {e}")))?;
        buf
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| Error::Misuse(format!("cannot read --names-from {path}: {e}")))?
    };
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        if ArtifactName::parse(name).is_err() {
            return Err(Error::Misuse(format!(
                "--names-from {path} line {}: '{name}' is not a valid name",
                n + 1
            )));
        }
        out.push(name.to_owned());
    }
    Ok(out)
}

/// Resolve the enumeration source of `down`, `diff`, `rm`, `mirror` and `mv`:
/// repeatable `--name`, repeatable `--names-from`, or `--ls`.
/// Without any source the call refuses (exit 1) with the enumeration hint.
pub(crate) fn enumeration_source(
    base: &str,
    names: &[String],
    names_from: &[String],
    ls: bool,
) -> Result<Enumeration, Error> {
    if ls {
        // `--ls` is a whole-listing source, not a filter: a destructive command
        // that also names files would act on the listing, not on the names.
        if !names.is_empty() || !names_from.is_empty() {
            return Err(Error::Misuse(
                "--ls cannot be combined with --name/--names-from: pass one enumeration source"
                    .to_owned(),
            ));
        }
        return Ok(Enumeration::Search);
    }
    let mut all: Vec<String> = names.to_vec();
    for path in names_from {
        all.extend(read_names_file(path)?);
    }
    if !all.is_empty() {
        return Ok(Enumeration::Names(parse_names(&all)?));
    }
    Err(Error::Enumerate {
        url: base.to_owned(),
        reason: "no --name/--names-from/--ls given".into(),
    })
}

/// Escape control characters in server-supplied text before a terminal sees it.
/// `--json` stays raw: serde already escapes control characters.
pub(crate) fn printable(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}

/// One JSON line for the simple (non-event) results.
pub(crate) fn print_line(json: bool, human: &str, value: &serde_json::Value) {
    if json {
        println!("{value}");
    } else {
        println!("{human}");
    }
}
