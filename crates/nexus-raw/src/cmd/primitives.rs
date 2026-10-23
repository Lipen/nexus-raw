//! The L0 commands: get, put, head, sha.

use std::path::Path;

use nexus_raw_core::{Error, ShaSource};

use crate::cmd::{finish, make_ctx, print_line, Ctx};
use crate::Cli;

pub(crate) async fn get(
    cli: &Cli,
    url: &str,
    out: Option<&Path>,
    cont: bool,
    segments: Option<usize>,
    range: Option<&str>,
) -> Result<(), Error> {
    if out.is_none() && cli.json {
        return Err(Error::Misuse(
            "--json with stdout body would corrupt the artifact: pass -o FILE, or drop --json"
                .to_owned(),
        ));
    }
    if segments.is_some() && out.is_none() {
        return Err(Error::Misuse(
            "--segments writes a file, not a stream: pass -o FILE, or drop --segments".to_owned(),
        ));
    }
    if segments.is_some() && cont {
        return Err(Error::Misuse(
            "--continue resumes the single-stream part file: --segments stages its own spans, pass --continue or --segments, not both"
                .to_owned(),
        ));
    }
    if cont && out.is_none() {
        return Err(Error::Misuse(
            "--continue resumes a sibling part file: pass -o FILE, or drop --continue".to_owned(),
        ));
    }
    if range.is_some() && (cont || segments.is_some()) {
        return Err(Error::Misuse(
            "--range reads one span: pass --range without --continue or --segments".to_owned(),
        ));
    }
    let ctx = make_ctx(cli, url)?;
    // The renderer drains on every path: the events the run already emitted must reach the output before the failure is reported.
    let result = run_get(&ctx, url, out, cont, segments, range).await;
    finish(ctx).await;
    result
}

async fn run_get(
    ctx: &Ctx,
    url: &str,
    out: Option<&Path>,
    cont: bool,
    segments: Option<usize>,
    range: Option<&str>,
) -> Result<(), Error> {
    if let Some(spec) = range {
        return run_get_range(ctx, url, out, spec).await;
    }
    let outcome = match segments {
        Some(n) => {
            ctx.nxr
                .get_segmented(
                    url,
                    out.expect("segments require a file target").to_owned(),
                    n,
                )
                .await?
        }
        None => ctx.nxr.get(url, out.map(Path::to_owned), cont).await?,
    };
    report_get(ctx, url, out, &outcome);
    Ok(())
}

pub(crate) async fn put(
    cli: &Cli,
    url: &str,
    file: Option<&Path>,
    stdin: bool,
    sha: bool,
) -> Result<(), Error> {
    let ctx = make_ctx(cli, url)?;
    let result = if stdin {
        run_put_stdin(&ctx, url, sha).await
    } else {
        match file {
            Some(f) => run_put(&ctx, url, f, sha).await,
            None => Err(Error::Misuse(
                "put needs -f FILE or --stdin: nothing to send".to_owned(),
            )),
        }
    };
    finish(ctx).await;
    result
}

/// `put --stdin`: read the whole body from stdin, then send it as one PUT.
/// The protocol carries Content-Length and the resume scheme assumes a known
/// size, so a pipe is buffered whole before the upload.
/// The digest rides the same buffered body.
async fn run_put_stdin(ctx: &Ctx, url: &str, sha: bool) -> Result<(), Error> {
    use std::io::Read;
    let mut body = Vec::new();
    std::io::stdin()
        .read_to_end(&mut body)
        .map_err(|e| Error::Io {
            path: "-".to_owned(),
            detail: e.to_string(),
        })?;
    let (size, digest) = ctx.nxr.put_bytes(url, &body, sha).await?;
    match digest {
        Some(d) => print_line(
            ctx.json,
            &format!("put: {size} bytes + marker {d} → {url}"),
            &serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": d.as_str()}),
        ),
        None => print_line(
            ctx.json,
            &format!("put: {size} bytes → {url} (no marker)"),
            &serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": serde_json::Value::Null}),
        ),
    }
    Ok(())
}

async fn run_put(ctx: &Ctx, url: &str, file: &Path, sha: bool) -> Result<(), Error> {
    let (size, digest) = ctx.nxr.put(url, file, sha).await?;
    match digest {
        Some(d) => print_line(
            ctx.json,
            &format!("put: {size} bytes + marker {d} → {url}"),
            &serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": d.as_str()}),
        ),
        None => print_line(
            ctx.json,
            &format!("put: {size} bytes → {url} (no marker)"),
            &serde_json::json!({"ok": true, "url": url, "bytes": size, "marker": serde_json::Value::Null}),
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
    // The content type is a server-supplied string: escape it for the terminal,
    // the same invariant the error paths keep against ANSI/OSC injection.
    let content_type = info
        .content_type
        .as_deref()
        .map(|ct| ct.escape_debug().to_string())
        .unwrap_or_else(|| "-".into());
    print_line(
        ctx.json,
        &format!(
            "head: {} {} {}",
            info.status,
            info.size.map_or("-".into(), |s| s.to_string()),
            content_type
        ),
        &serde_json::json!({
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
        // A scheme that is not http(s) is a command-line mistake, not a
        // missing local file: the exit code table promises 2 for both.
        if let Some((scheme, _)) = target.split_once("://") {
            return Err(Error::Misuse(format!(
                "sha takes an http(s) URL or a local path, got a {scheme} URL"
            )));
        }
        let path = std::path::PathBuf::from(target);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(Error::Misuse(format!("not a file: {}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Misuse(format!(
                    "no such local file or http(s) URL: {target}"
                )))
            }
            Err(e) => return Err(Error::io(&path, e)),
        }
        let d = nexus_raw_core::model::digest::sha256_file(&path).map_err(|e| Error::Io {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
        print_line(
            cli.json,
            d.as_str(),
            &serde_json::json!({"sha256": d.as_str(), "source": target}),
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
        d.as_str(),
        &serde_json::json!({"sha256": d.as_str(), "source": target}),
    );
    Ok(())
}

async fn run_get_range(ctx: &Ctx, url: &str, out: Option<&Path>, spec: &str) -> Result<(), Error> {
    let range = ctx.nxr.get_range(url, spec).await?;
    let label = match range.end {
        Some(end) => format!("{}-{end}", range.start),
        None => format!("{}-", range.start),
    };
    match out {
        Some(path) => {
            nexus_raw_core::primitive::write_bytes_atomically(path, &range.bytes)?;
            print_line(
                ctx.json,
                &format!(
                    "range {label}: {} bytes → {}",
                    range.bytes.len(),
                    path.display()
                ),
                &serde_json::json!({
                    "ok": true,
                    "url": url,
                    "out": path.display().to_string(),
                    "range": label,
                    "bytes": range.bytes.len(),
                }),
            );
        }
        None => {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&range.bytes).map_err(|e| Error::Io {
                path: "-".to_owned(),
                detail: e.to_string(),
            })?;
            stdout.flush().map_err(|e| Error::Io {
                path: "-".to_owned(),
                detail: e.to_string(),
            })?;
        }
    }
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
                &format!("get: {} bytes → {}{}", outcome.size, p.display(), resumed),
                &serde_json::json!({
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
