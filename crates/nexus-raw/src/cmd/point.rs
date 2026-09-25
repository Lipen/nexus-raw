//! `nxr point`: atomically move a pointer to a version.

use nexus_raw_core::{Error, Nxr, PointOutcome};
use serde_json::json;

use crate::cmd;
use crate::config_setup::build_config;
use crate::Cli;

pub(crate) async fn run(
    cli: &Cli,
    pointer: &str,
    version: &str,
    if_newer: bool,
) -> Result<(), Error> {
    let cfg = build_config(cli, true)?;
    // No events are emitted; keep the receiver alive so sends never fail.
    let (tx, _rx) = cmd::event_channel();
    let nxr = Nxr::new(cfg, tx)?;
    match nxr.point(pointer, version, if_newer).await? {
        PointOutcome::Written { from } => {
            if cli.json {
                let line = json!({
                    "event": "point",
                    "outcome": "written",
                    "pointer": pointer,
                    "version": version,
                    "from": from,
                });
                println!("{line}");
            } else {
                match from {
                    Some(prev) => println!("pointed {pointer} at {version} (was {prev})"),
                    None => println!("pointed {pointer} at {version} (was unset)"),
                }
            }
        }
        PointOutcome::Skipped { current } => {
            if cli.json {
                let line = json!({
                    "event": "point",
                    "outcome": "skipped",
                    "pointer": pointer,
                    "current": current,
                });
                println!("{line}");
            } else {
                println!("skipped: {pointer} already at {current}");
            }
        }
    }
    Ok(())
}
