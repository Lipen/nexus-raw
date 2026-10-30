//! The L2 commands: ls, channel, verify.

use std::path::Path;

use nexus_raw_core::{Error, Summary};

use crate::cmd::{finish, make_ctx, print_line, printable, Ctx};
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
        let (names, skipped) = ctx.nxr.ls_assets_counted().await?;
        for n in &names {
            print_line(
                ctx.json,
                &printable(n.as_str()),
                &serde_json::json!({"name": n.to_string()}),
            );
        }
        if skipped > 0 {
            // A name outside the artifact grammar cannot be transferred by the
            // protocol: naming the count keeps the undercount visible.
            print_line(
                ctx.json,
                &format!(
                    "{skipped} name(s) outside the artifact grammar hidden (nxr cannot transfer them)"
                ),
                &serde_json::json!({"event": "hidden", "count": skipped, "reason": "grammar"}),
            );
        }
    } else {
        let (entries, hidden) = ctx.nxr.ls_entries_counted().await?;
        for e in &entries {
            match e.kind {
                nexus_raw_core::EntryKind::Dir => {
                    let name = format!("{}/", printable(&e.name));
                    print_line(
                        ctx.json,
                        &name,
                        &serde_json::json!({"entry": e.name, "kind": "dir"}),
                    );
                }
                nexus_raw_core::EntryKind::File => {
                    // The search answer already carried the facts: a consumer
                    // plans without a HEAD per name.
                    let mut json = serde_json::json!({"entry": e.name, "kind": "file"});
                    if let Some(size) = e.size {
                        json["size"] = serde_json::json!(size);
                    }
                    if let Some(digest) = &e.digest {
                        json["sha256"] = serde_json::json!(digest.as_str());
                    }
                    print_line(ctx.json, &printable(&e.name), &json);
                }
            }
        }
        if hidden > 0 {
            // The enumerator hides markers by design: naming the count keeps
            // the undercount visible instead of a silent surprise.
            print_line(
                ctx.json,
                &format!("{hidden} marker objects hidden (fetch one by adding .sha256)"),
                &serde_json::json!({"event": "hidden", "count": hidden, "reason": "marker"}),
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

/// `point <URL>`: DELETE the pointer file.
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

pub(crate) async fn verify(cli: &Cli, dir: &Path) -> Result<(), Error> {
    // verify needs no network: the base is only a placeholder for the client.
    let ctx = make_ctx(cli, "http://localhost/")?;
    let result = run_verify(&ctx, dir).await;
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

async fn run_verify(ctx: &Ctx, dir: &Path) -> Result<Summary, Error> {
    // The verdict line is printed by the caller, after the renderer drained.
    ctx.nxr.verify(dir, None).await
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
        .map(|r| {
            format!(
                "{} {} {} {}",
                printable(&r.name),
                printable(&r.format),
                printable(&r.kind),
                printable(&r.url)
            )
        })
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
        printable(&report.root),
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
        human.push_str(&format!(", version {}", printable(v)));
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
            printable(&repo.info.name),
            printable(&repo.info.format),
            printable(&repo.info.kind),
            printable(&repo.info.url),
            serde_json::to_string_pretty(d).unwrap_or_default()
        ),
        None => format!(
            "{} {} {} {}",
            printable(&repo.info.name),
            printable(&repo.info.format),
            printable(&repo.info.kind),
            printable(&repo.info.url)
        ),
    };
    print_line(
        ctx.json,
        &human,
        &serde_json::to_value(&repo).unwrap_or_default(),
    );
    Ok(())
}

/// Every asset of the repository (spec §3): one JSON object `{"assets":[…],"summary":{…}}`, not an event stream.
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
                    .map(|s| s.get(..8).unwrap_or(s))
                    .unwrap_or("-");
                let size = a.size.map_or_else(|| "-".to_owned(), |s| s.to_string());
                format!(
                    "{}  {}  {}  {}",
                    size,
                    printable(sha),
                    printable(a.last_modified.as_deref().unwrap_or("-")),
                    printable(&a.path)
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
            printable(
                status
                    .disclaimer
                    .split(". ")
                    .next()
                    .unwrap_or(&status.disclaimer)
            )
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
