//! The L2 commands: ls, channel, verify.

use std::path::Path;

use nexus_raw_core::{Error, Manifest};

use crate::cmd::{finish, make_ctx, print_line};
use crate::Cli;

pub(crate) async fn ls(cli: &Cli, url: &str, assets: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    if assets {
        let names = ctx.nxr.ls_assets().await?;
        for n in &names {
            print_line(
                ctx.json,
                n.to_string(),
                serde_json::json!({"name": n.to_string()}),
            );
        }
    } else {
        let versions = ctx.nxr.ls_versions().await?;
        for v in &versions {
            print_line(ctx.json, v.clone(), serde_json::json!({"version": v}));
        }
    }
    finish(ctx).await;
    Ok(())
}

pub(crate) async fn channel_get(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let token = ctx.nxr.channel_get(url).await?;
    match token {
        Some(t) => print_line(
            ctx.json,
            t.clone(),
            serde_json::json!({"url": url, "token": t}),
        ),
        None => print_line(
            ctx.json,
            "unset".to_owned(),
            serde_json::json!({"url": url, "token": serde_json::Value::Null}),
        ),
    }
    finish(ctx).await;
    Ok(())
}

pub(crate) async fn channel_set(
    cli: &Cli,
    url: &str,
    token: &str,
    if_forward: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let outcome = ctx.nxr.channel_set(url, token, if_forward).await?;
    match outcome {
        nexus_raw_core::ChannelOutcome::Written { from } => print_line(
            ctx.json,
            format!("channel: set {url} → {token}"),
            serde_json::json!({"url": url, "outcome": "written", "token": token, "from": from}),
        ),
        nexus_raw_core::ChannelOutcome::Skipped { current } => print_line(
            ctx.json,
            format!("channel: kept {url} at {current} (forward-only)"),
            serde_json::json!({"url": url, "outcome": "skipped", "current": current}),
        ),
    }
    finish(ctx).await;
    Ok(())
}

pub(crate) async fn verify(cli: &Cli, dir: &Path, manifest: Option<&str>) -> Result<(), Error> {
    // verify needs no network: the base is only a placeholder for the client.
    let ctx = make_ctx(cli, "http://localhost/")?;
    let names = match manifest {
        Some(spec) => {
            let m: Manifest = if spec == "-" {
                Manifest::from_stdin()?
            } else {
                Manifest::from_file(Path::new(spec))?
            };
            Some(m.names)
        }
        None => None,
    };
    let summary = ctx.nxr.verify(dir, names).await?;
    if !ctx.json {
        println!(
            "verify: {} ok, FAILED: {}",
            summary.skipped,
            if summary.failed.is_empty() {
                "none".to_owned()
            } else {
                summary.failed.join(", ")
            }
        );
    }
    finish(ctx).await;
    Ok(())
}
