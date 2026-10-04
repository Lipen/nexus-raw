//! The L2 commands: ls, channel, verify.

use std::path::Path;

use nexus_raw_core::{Error, Manifest, Summary};

use crate::cmd::{finish, make_ctx, print_line, Ctx};
use crate::Cli;

pub(crate) async fn ls(cli: &Cli, url: &str, assets: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
    let result = run_ls(&ctx, assets).await;
    finish(ctx).await;
    result
}

async fn run_ls(ctx: &Ctx, assets: bool) -> Result<(), Error> {
    if assets {
        let names = ctx.nxr.ls_assets().await?;
        for n in &names {
            print_line(
                ctx.json,
                n.as_str(),
                &serde_json::json!({"name": n.to_string()}),
            );
        }
    } else {
        let entries = ctx.nxr.ls_entries().await?;
        for e in &entries {
            match e.kind {
                nexus_raw_core::EntryKind::Dir => {
                    let name = format!("{}/", e.name);
                    print_line(
                        ctx.json,
                        &name,
                        &serde_json::json!({"entry": e.name, "kind": "dir"}),
                    );
                }
                nexus_raw_core::EntryKind::File => {
                    print_line(
                        ctx.json,
                        &e.name,
                        &serde_json::json!({"entry": e.name, "kind": "file"}),
                    );
                }
            }
        }
    }
    Ok(())
}

pub(crate) async fn channel_get(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_channel_get(&ctx, url).await;
    finish(ctx).await;
    result
}

/// `point --clear <URL>`: DELETE the pointer file.
pub(crate) async fn point_clear(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_point_clear(&ctx, url).await;
    finish(ctx).await;
    result
}

async fn run_point_clear(ctx: &Ctx, url: &str) -> Result<(), Error> {
    match ctx.nxr.point_clear(url).await? {
        nexus_raw_core::ClearOutcome::Cleared => print_line(
            ctx.json,
            &format!("point: cleared {url}"),
            &serde_json::json!({"url": url, "outcome": "cleared"}),
        ),
        nexus_raw_core::ClearOutcome::Absent => print_line(
            ctx.json,
            &format!("point: absent {url}"),
            &serde_json::json!({"url": url, "outcome": "absent"}),
        ),
    }
    Ok(())
}

async fn run_channel_get(ctx: &Ctx, url: &str) -> Result<(), Error> {
    let token = ctx.nxr.channel_get(url).await?;
    match token {
        Some(t) => {
            let payload = serde_json::json!({"url": url, "token": t});
            print_line(ctx.json, &t, &payload);
        }
        None => print_line(
            ctx.json,
            "unset",
            &serde_json::json!({"url": url, "token": serde_json::Value::Null}),
        ),
    }
    Ok(())
}

pub(crate) async fn channel_set(
    cli: &Cli,
    url: &str,
    token: &str,
    if_forward: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_channel_set(&ctx, url, token, if_forward).await;
    finish(ctx).await;
    result
}

async fn run_channel_set(ctx: &Ctx, url: &str, token: &str, if_forward: bool) -> Result<(), Error> {
    let outcome = ctx.nxr.channel_set(url, token, if_forward).await?;
    match outcome {
        nexus_raw_core::ChannelOutcome::Written { from } => print_line(
            ctx.json,
            &format!("channel: set {url} → {token}"),
            &serde_json::json!({"url": url, "outcome": "written", "token": token, "from": from}),
        ),
        nexus_raw_core::ChannelOutcome::Skipped { current } => print_line(
            ctx.json,
            &format!("channel: kept {url} at {current} (forward-only)"),
            &serde_json::json!({"url": url, "outcome": "skipped", "current": current}),
        ),
    }
    Ok(())
}

pub(crate) async fn verify(cli: &Cli, dir: &Path, manifest: Option<&str>) -> Result<(), Error> {
    // verify needs no network: the base is only a placeholder for the client.
    let ctx = make_ctx(cli, "http://localhost/")?;
    let result = run_verify(&ctx, dir, manifest).await;
    // The verdict prints after the renderer drained: the summary line always
    // precedes it, in both failure and success, no task race.
    finish(ctx).await;
    // The core returns Err(Incomplete) naming the failures on any verdict but clean:
    // that path surfaces through the error rendering, the line here is the clean verdict.
    let summary = result?;
    if !cli.json {
        println!("verify: {} ok, FAILED: none", summary.skipped);
    }
    Ok(())
}

async fn run_verify(ctx: &Ctx, dir: &Path, manifest: Option<&str>) -> Result<Summary, Error> {
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
    // The verdict line is printed by the caller, after the renderer drained.
    ctx.nxr.verify(dir, names).await
}

/// List the repositories of the server behind `url`: the service REST API, not storage protocol.
/// The URL may be the server root or any repository URL: both root to the same server.
pub(crate) async fn service_repos(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
    let result = run_service_repos(&ctx).await;
    finish(ctx).await;
    result
}

async fn run_service_repos(ctx: &Ctx) -> Result<(), Error> {
    let repos = ctx.nxr.service_repos().await?;
    let human = repos
        .iter()
        .map(|r| format!("{} {} {} {}", r.name, r.format, r.kind, r.url))
        .collect::<Vec<_>>()
        .join("\n");
    print_line(ctx.json, &human, &serde_json::json!({ "repos": repos }));
    Ok(())
}
