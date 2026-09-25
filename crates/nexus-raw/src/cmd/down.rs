//! `nxr down`: fetch a version, or a subset of it, into a directory.

use std::path::Path;

use nexus_raw_core::{ArtifactName, Claim, Error, Event, Nxr};
use tokio::sync::mpsc::UnboundedSender;

use crate::config_setup::build_config;
use crate::render::{Mode, Session};
use crate::{cmd, Cli};

pub(crate) async fn run(
    cli: &Cli,
    dir: &Path,
    version: Option<&str>,
    pointer: Option<&str>,
    only: &[String],
    names: Option<&Path>,
) -> Result<(), Error> {
    match (version, pointer) {
        (Some(_), Some(_)) => {
            return Err(Error::Misuse(
                "--version and --pointer are mutually exclusive".into(),
            ));
        }
        (None, None) => {
            return Err(Error::Misuse(
                "one of --version or --pointer is required".into(),
            ));
        }
        _ => {}
    }
    // Parse and validate the filter before touching the network.
    let only = cmd::only_filter(only, names.map(Claim::read))?;
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let result = execute(
        cli,
        dir,
        version,
        pointer,
        only.as_deref(),
        &session.sender(),
    )
    .await;
    session.finish().await;
    result
}

async fn execute(
    cli: &Cli,
    dir: &Path,
    version: Option<&str>,
    pointer: Option<&str>,
    only: Option<&[ArtifactName]>,
    tx: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let cfg = build_config(cli, true)?;
    let base = cfg.base.clone();
    let nxr = Nxr::new(cfg, tx.clone())?;
    let version = match pointer {
        Some(p) => nxr.resolve_pointer(p).await?,
        None => version
            .expect("version checked in run: exactly one of version/pointer")
            .to_owned(),
    };
    let claim = match nxr.read_remote_claim(&version).await? {
        Some(c) => c,
        // Missing claim is a 404 at its url.
        None => {
            return Err(Error::Http {
                status: 404,
                url: format!("{base}{version}/claim.json"),
            });
        }
    };
    nxr.down(dir, &claim, only, None).await?;
    Ok(())
}
