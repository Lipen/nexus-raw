//! `nxr diff`: the symmetric plan against the server; nothing is written.

use std::path::Path;

use nexus_raw_core::{Claim, Error, Event, Nxr};
use tokio::sync::mpsc::UnboundedSender;

use crate::config_setup::build_config;
use crate::render::{Mode, Session};
use crate::{cmd, Cli};

pub(crate) async fn run(cli: &Cli, dir: &Path, names: Option<&Path>) -> Result<(), Error> {
    cmd::require_dir(dir)?;
    let claim = cmd::load_claim(dir, names)?;
    let session = Session::start(Mode::from_flags(cli.json, cli.quiet, cli.verbose));
    let result = execute(cli, dir, &claim, &session.sender()).await;
    session.finish().await;
    result
}

async fn execute(
    cli: &Cli,
    dir: &Path,
    claim: &Claim,
    tx: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let cfg = build_config(cli, true)?;
    let nxr = Nxr::new(cfg, tx.clone())?;
    let actions = nxr.diff(dir, claim).await?;
    cmd::send_plan(tx, &actions)?;
    Ok(())
}
