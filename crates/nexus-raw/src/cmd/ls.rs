//! `nxr ls`: per-name remote states, or the version list.

use nexus_raw_core::{Error, Nxr, RemoteStatus};
use serde_json::json;

use crate::cmd;
use crate::config_setup::build_config;
use crate::Cli;

pub(crate) async fn run(cli: &Cli, version: Option<&str>) -> Result<(), Error> {
    let cfg = build_config(cli, true)?;
    // These calls emit no events; keep the receiver alive so sends never fail.
    let (tx, _rx) = cmd::event_channel();
    let nxr = Nxr::new(cfg, tx)?;
    match version {
        Some(v) => {
            let rows = nxr.remote_state(v).await?;
            print_states(cli.json, &rows);
        }
        None => {
            let versions = nxr.ls_versions().await?;
            print_versions(cli.json, &versions);
        }
    }
    Ok(())
}

fn print_states(json: bool, rows: &[(nexus_raw_core::ArtifactName, RemoteStatus)]) {
    if json {
        for (name, state) in rows {
            let (state, size, digest) = match state {
                RemoteStatus::Complete { digest, size } => ("complete", *size, Some(digest.to_string())),
                RemoteStatus::Markerless { size } => ("markerless", *size, None),
                RemoteStatus::Absent => ("absent", None, None),
                RemoteStatus::Broken(_) => ("broken", None, None),
            };
            let line = json!({
                "name": name.as_str(),
                "state": state,
                "size": size,
                "digest": digest,
            });
            println!("{line}");
        }
        return;
    }
    let width = rows.iter().map(|(n, _)| n.as_str().len()).max().unwrap_or(4);
    println!("{:<width$}  {:<10}  size", "name", "state", width = width);
    for (name, state) in rows {
        let (label, size) = match state {
            RemoteStatus::Complete { size, .. } => ("complete", size.as_ref()),
            RemoteStatus::Markerless { size } => ("markerless", size.as_ref()),
            RemoteStatus::Absent => ("absent", None),
            RemoteStatus::Broken(_) => ("broken", None),
        };
        let size = match size {
            Some(s) => s.to_string(),
            None => "-".into(),
        };
        println!("{:<width$}  {:<10}  {}", name.as_str(), label, size, width = width);
    }
}

fn print_versions(json: bool, versions: &[String]) {
    for v in versions {
        if json {
            let line = json!({ "version": v });
            println!("{line}");
        } else {
            println!("{v}");
        }
    }
}
