//! Command handlers and the small helpers they share.

mod diff;
mod down;
mod ls;
mod point;
mod up;
mod verify;

use std::path::Path;

use nexus_raw_core::{Action, ArtifactName, Claim, Error, Event};
use tokio::sync::mpsc::UnboundedSender;

use crate::Cmd;

pub(crate) async fn dispatch(cli: &crate::Cli) -> Result<(), Error> {
    match &cli.command {
        Cmd::Up {
            dir,
            names,
            dry_run,
        } => up::run(cli, dir, *dry_run, names.as_deref()).await,
        Cmd::Down {
            dir,
            version,
            pointer,
            only,
            names,
        } => {
            down::run(
                cli,
                dir,
                version.as_deref(),
                pointer.as_deref(),
                only,
                names.as_deref(),
            )
            .await
        }
        Cmd::Verify { dir, names } => verify::run(cli, dir, names.as_deref()).await,
        Cmd::Diff { dir, names } => diff::run(cli, dir, names.as_deref()).await,
        Cmd::Ls { version } => ls::run(cli, version.as_deref()).await,
        Cmd::Point {
            pointer,
            version,
            if_newer,
        } => point::run(cli, pointer, version, *if_newer).await,
    }
}

/// Read the claim from `names` or, by default, from `<dir>/claim.json`.
pub(crate) fn load_claim(dir: &Path, names: Option<&Path>) -> Result<Claim, Error> {
    match names {
        Some(file) => Claim::read(file),
        None => Claim::read(&Claim::path_in(dir)),
    }
}

/// Refuse a missing working directory up front with a misuse error.
pub(crate) fn require_dir(dir: &Path) -> Result<(), Error> {
    if dir.is_dir() {
        Ok(())
    } else {
        Err(Error::Misuse(format!("not a directory: {}", dir.display())))
    }
}

/// Union of `--only` values and the artifacts of the `--names` claim file.
///
/// `Ok(None)` means no restriction; every parsed name goes through
/// [`ArtifactName::parse`] so unsafe names fail with exit code 2.
pub(crate) fn only_filter(
    only: &[String],
    names_claim: Option<Result<Claim, Error>>,
) -> Result<Option<Vec<ArtifactName>>, Error> {
    if only.is_empty() && names_claim.is_none() {
        return Ok(None);
    }
    let mut filter: Vec<ArtifactName> = only
        .iter()
        .map(|s| ArtifactName::parse(s))
        .collect::<Result<_, _>>()?;
    if let Some(claim) = names_claim {
        filter.extend(claim?.artifacts);
    }
    Ok(Some(filter))
}

/// Partition actions into upload/download/skip name lists and emit a plan event.
///
/// Used by `diff` and `up --dry-run`, where the core computes the plan but does
/// not emit the event itself.
pub(crate) fn send_plan(tx: &UnboundedSender<Event>, actions: &[Action]) -> Result<(), Error> {
    let mut upload = Vec::new();
    let mut download = Vec::new();
    let mut skip = Vec::new();
    for action in actions {
        match action {
            Action::Upload { name, .. } => upload.push(name.as_str().to_owned()),
            Action::Download { name, .. } => download.push(name.as_str().to_owned()),
            Action::Skip { name, .. } => skip.push(name.as_str().to_owned()),
        }
    }
    tx.send(Event::Plan {
        upload,
        download,
        skip,
    })
    .map_err(|_| Error::Misuse("event channel closed".into()))
}

/// Convenience for commands whose core calls emit no events: keep the receiver
/// alive so the channel never looks closed.
pub(crate) fn event_channel() -> (
    UnboundedSender<Event>,
    tokio::sync::mpsc::UnboundedReceiver<Event>,
) {
    tokio::sync::mpsc::unbounded_channel()
}
