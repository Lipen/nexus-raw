//! The L0 commands: get, put, head, sha.

use std::path::Path;

use nexus_raw_core::{Error, ShaSource};

use crate::cmd::{finish, make_ctx, print_line, Ctx};
use crate::Cli;

pub(crate) async fn get(cli: &Cli, url: &str, out: Option<&Path>, cont: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    // The renderer drains on every path: the events the run already emitted
    // must reach the output before the failure is reported.
    let result = run_get(&ctx, url, out, cont).await;
    finish(ctx).await;
    result
}

async fn run_get(ctx: &Ctx, url: &str, out: Option<&Path>, cont: bool) -> Result<(), Error> {
    let outcome = ctx.nxr.get(url, out.map(Path::to_owned), cont).await?;
    report_get(ctx, url, out, &outcome);
    Ok(())
}

pub(crate) async fn put(cli: &Cli, url: &str, file: &Path, sha: bool) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_put(&ctx, url, file, sha).await;
    finish(ctx).await;
    result
}

async fn run_put(ctx: &Ctx, url: &str, file: &Path, sha: bool) -> Result<(), Error> {
    let (size, digest) = ctx.nxr.put(url, file, sha).await?;
    match digest {
        Some(d) => print_line(
            ctx.json,
            format!("put: {size} bytes + marker {d} → {url}"),
            serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": d.as_str()}),
        ),
        None => print_line(
            ctx.json,
            format!("put: {size} bytes → {url} (no marker)"),
            serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": serde_json::Value::Null}),
        ),
    }
    Ok(())
}

pub(crate) async fn head(cli: &Cli, url: &str) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = run_head(&ctx, url).await;
    finish(ctx).await;
    result
}

async fn run_head(ctx: &Ctx, url: &str) -> Result<(), Error> {
    let info = ctx.nxr.head(url).await?;
    print_line(
        ctx.json,
        format!(
            "head: {} {} {}",
            info.status,
            info.size
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into()),
            info.content_type.clone().unwrap_or_else(|| "-".into()),
        ),
        serde_json::json!({
            "url": url,
            "status": info.status,
            "size": info.size,
            "content_type": info.content_type,
        }),
    );
    Ok(())
}

pub(crate) async fn sha(cli: &Cli, target: &str) -> Result<(), Error> {
    // A local file needs no HTTP client at all.
    if !target.starts_with("http://") && !target.starts_with("https://") {
        let path = std::path::PathBuf::from(target);
        let d = nexus_raw_core::model::digest::sha256_file(&path)
            .map_err(|e| Error::Misuse(format!("{}: {e}", path.display())))?;
        print_line(
            cli.json,
            d.as_str().to_owned(),
            serde_json::json!({"sha256": d.as_str(), "source": target}),
        );
        return Ok(());
    }
    let ctx = make_ctx(cli, target)?;
    let result = run_sha(&ctx, target).await;
    finish(ctx).await;
    result
}

async fn run_sha(ctx: &Ctx, target: &str) -> Result<(), Error> {
    let d = ctx.nxr.sha(ShaSource::Url(target.to_owned())).await?;
    print_line(
        ctx.json,
        d.as_str().to_owned(),
        serde_json::json!({"sha256": d.as_str(), "source": target}),
    );
    Ok(())
}

fn report_get(ctx: &Ctx, url: &str, out: Option<&Path>, outcome: &nexus_raw_core::GetOutcome) {
    match out {
        Some(p) => {
            let resumed = if outcome.resumed_from > 0 {
                format!(" (resumed from {})", outcome.resumed_from)
            } else {
                String::new()
            };
            print_line(
                ctx.json,
                format!("get: {} bytes → {}{}", outcome.size, p.display(), resumed),
                serde_json::json!({
                    "ok": true,
                    "url": url,
                    "out": p.display().to_string(),
                    "bytes": outcome.size,
                    "resumed_from": outcome.resumed_from,
                    "sha256": outcome.digest.as_ref().map(nexus_raw_core::Digest::as_str),
                }),
            );
        }
        None => {
            // Stdout carries the body: the only report is a stderr line at -v.
            if !ctx.json {
                log::debug!("get: {} bytes from {url}", outcome.size);
            }
        }
    }
}
