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
        let (entries, hidden) = ctx.nxr.ls_entries_counted().await?;
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
        if hidden > 0 {
            // The enumerator hides markers by design: naming the count keeps
            // the undercount visible instead of a silent surprise.
            print_line(
                ctx.json,
                &format!("{hidden} marker objects hidden (fetch one by adding .sha256)"),
                &serde_json::json!({"event": "hidden", "count": hidden}),
            );
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

/// Server liveness and writability (spec §1): the verdict plus the server version when it tells one.
pub(crate) async fn service_status(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_service_status(&ctx).await;
    finish(ctx).await;
    result
}

async fn run_service_status(ctx: &Ctx) -> Result<(), Error> {
    let report = ctx.nxr.service_status().await?;
    let mut human = format!(
        "{}: {}",
        report.root,
        if report.alive {
            "alive"
        } else {
            "no status endpoint (or the server is down)"
        }
    );
    if report.alive {
        human.push_str(if report.writable {
            ", writable"
        } else {
            ", read-only (or the writable probe was refused)"
        });
    }
    if let Some(v) = &report.version {
        human.push_str(&format!(", version {v}"));
    }
    print_line(
        ctx.json,
        &human,
        &serde_json::to_value(&report).unwrap_or_default(),
    );
    Ok(())
}

/// The repository behind the URL (spec §2): the visible entry, plus the admin detail with --detail.
pub(crate) async fn service_repo(cli: &Cli, url: &str, detail: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_service_repo(&ctx, detail).await;
    finish(ctx).await;
    result
}

async fn run_service_repo(ctx: &Ctx, detail: bool) -> Result<(), Error> {
    let repo = ctx.nxr.service_repo(detail).await?;
    let human = match &repo.detail {
        Some(d) => format!(
            "{} {} {} {} (full settings follow)\n{}",
            repo.info.name,
            repo.info.format,
            repo.info.kind,
            repo.info.url,
            serde_json::to_string_pretty(d).unwrap_or_default()
        ),
        None => format!(
            "{} {} {} {}",
            repo.info.name, repo.info.format, repo.info.kind, repo.info.url
        ),
    };
    print_line(
        ctx.json,
        &human,
        &serde_json::to_value(&repo).unwrap_or_default(),
    );
    Ok(())
}

/// Every asset of the repository (spec §3): one NDJSON line per asset, the summary last.
pub(crate) async fn service_assets(
    cli: &Cli,
    url: &str,
    q: Option<&str>,
    prefix: &[String],
) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_service_assets(&ctx, q, prefix).await;
    finish(ctx).await;
    result
}

async fn run_service_assets(ctx: &Ctx, q: Option<&str>, prefix: &[String]) -> Result<(), Error> {
    let (assets, summary) = ctx.nxr.service_assets(q, prefix).await?;
    let human = if assets.is_empty() {
        "no assets (an empty result may also mean the search index lags a recent write)".to_owned()
    } else {
        assets
            .iter()
            .map(|a| {
                let sha = a
                    .sha256
                    .as_deref()
                    .map(|s| &s[..8.min(s.len())])
                    .unwrap_or("-");
                let size = a.size.map_or_else(|| "-".to_owned(), |s| s.to_string());
                format!(
                    "{}  {}  {}  {}",
                    size,
                    sha,
                    a.last_modified.as_deref().unwrap_or("-"),
                    a.path
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    print_line(
        ctx.json,
        &human,
        &serde_json::json!({ "assets": assets, "summary": summary }),
    );
    Ok(())
}

/// The EULA gate (spec §4): read it, or open it with --accept.
pub(crate) async fn service_eula(cli: &Cli, url: &str, accept: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_service_eula(&ctx, accept).await;
    finish(ctx).await;
    result
}

async fn run_service_eula(ctx: &Ctx, accept: bool) -> Result<(), Error> {
    if !accept {
        let Some(status) = ctx.nxr.service_eula().await? else {
            print_line(
                ctx.json,
                "no EULA gate on this server",
                &serde_json::json!({ "gate": false }),
            );
            return Ok(());
        };
        let human = format!(
            "accepted: {}; disclaimer: {}",
            status.accepted,
            status
                .disclaimer
                .split(". ")
                .next()
                .unwrap_or(&status.disclaimer)
        );
        print_line(
            ctx.json,
            &human,
            &serde_json::to_value(&status).unwrap_or_default(),
        );
        return Ok(());
    }
    let outcome = ctx.nxr.service_eula_accept().await?;
    let human = if outcome.accepted {
        format!("EULA accepted ({})", outcome.action)
    } else {
        "no EULA gate on this server".to_owned()
    };
    print_line(
        ctx.json,
        &human,
        &serde_json::to_value(&outcome).unwrap_or_default(),
    );
    Ok(())
}
