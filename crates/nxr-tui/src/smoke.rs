//! `--smoke`: the browse-and-download flow without the terminal.
//! Lists the repositories, walks the tree two levels deep, downloads one
//! subtree and prints the summary. The same server resolution as the TUI:
//! positional URLs, `--server` presets, or the config file.

use std::path::Path;

use anyhow::{bail, Context};
use nexus_raw_core::{Entry, EntryKind, Enumeration, Event, Nxr};
use tokio::sync::mpsc;

use crate::args::Args;
use crate::config::ConfigFile;
use crate::net;

/// Runs the headless browse-and-download flow.
///
/// # Errors
///
/// Returns an error when the bootstrap, any listing, the walk or the download fails.
pub async fn run(args: &Args, cfg: ConfigFile) -> anyhow::Result<()> {
    let auth = net::auth_header(&args.user)?;
    let servers = args.resolve_servers(&cfg)?;

    // The same fail-fast bootstrap the TUI uses: every server must answer.
    let mut all = Vec::new();
    for server in &servers {
        let repos = net::server_repos(server, auth.clone())
            .await
            .with_context(|| format!("server {}", server.name))?;
        println!("server {}", server.url);
        for repo in &repos {
            println!("  {:<12} {:<7} {}", repo.name, repo.format, repo.kind);
            all.push(repo.clone());
        }
    }

    // A hosted raw repository accepts uploads and holds files: the download
    // target of choice. The first plain raw repository is the fallback.
    let picked = all
        .iter()
        .find(|r| r.format == "raw" && r.kind == "hosted")
        .or_else(|| all.iter().find(|r| r.format == "raw"))
        .cloned();
    let Some(repo) = picked else {
        bail!("no raw repository found on any server");
    };
    let repo_url = net::dir_url(&repo.url);

    // The tree walk: the root listing, then two descents along the first folder.
    let root = listing(&repo_url, "", &auth).await?;
    println!("tree of {}:", repo.name);
    print_entries("", &root);
    let mut dir_rel = String::new();
    for level in 1..=2 {
        let Some(name) = listing(&repo_url, &dir_rel, &auth)
            .await?
            .into_iter()
            .find(|e| e.kind == EntryKind::Dir)
            .map(|e| e.name)
        else {
            break;
        };
        dir_rel = format!("{dir_rel}{name}/");
        let entries = listing(&repo_url, &dir_rel, &auth).await?;
        println!("level {level}: {dir_rel}");
        print_entries("  ", &entries);
    }

    // The subtree download: the first root folder, or the whole repo when it is flat.
    let sub_rel = root
        .iter()
        .find(|e| e.kind == EntryKind::Dir)
        .map(|e| format!("{}/", e.name))
        .unwrap_or_default();
    let names = net::collect_dir(&repo_url, &sub_rel, auth.clone()).await?;
    if names.is_empty() {
        bail!("no files under {sub_rel:?}");
    }
    let dst = Path::new("nxr-tui-smoke").join(&repo.name);
    // A rerun downloads for real instead of skipping everything already present.
    let _ = std::fs::remove_dir_all(&dst);
    println!(
        "plan: {} files -> {} (subtree {})",
        names.len(),
        dst.display(),
        if sub_rel.is_empty() {
            "<repo root>"
        } else {
            sub_rel.trim_end_matches('/')
        }
    );

    let (events, mut event_rx) = mpsc::unbounded_channel::<Event>();
    let printer = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                Event::Plan { download, skip, .. } => {
                    println!(
                        "diff: {} to download, {} already complete",
                        download.len(),
                        skip.len()
                    );
                }
                Event::ArtifactDone { name, skipped, .. } => {
                    let mark = if skipped { "skip" } else { "done" };
                    println!("  {mark} {name}");
                }
                Event::Retrying {
                    name,
                    attempt,
                    reason,
                } => {
                    println!("  retry {attempt} {name}: {reason}");
                }
                Event::Summary(_)
                | Event::ArtifactStarted { .. }
                | Event::ArtifactBytes { .. }
                | Event::Removing { .. }
                | Event::Removed { .. }
                | Event::Missing { .. } => {}
            }
        }
    });

    let summary = {
        let nxr = Nxr::new(net::config(repo_url.clone(), auth.clone()), events)?;
        nxr.down(&dst, Enumeration::Names(names), false)
            .await
            .context("download")?
    };
    let _ = printer.await;
    println!(
        "summary: downloaded {}, skipped {}, failed {}",
        summary.downloaded,
        summary.skipped,
        summary.failed.len()
    );
    if !summary.failed.is_empty() {
        bail!("failed names: {}", summary.failed.join(", "));
    }
    println!(
        "smoke ok: {} complete under {}",
        summary.downloaded,
        dst.display()
    );
    Ok(())
}

/// Lists one directory of the repository through `ls_entries`.
async fn listing(
    repo_url: &str,
    dir_rel: &str,
    auth: &Option<String>,
) -> anyhow::Result<Vec<Entry>> {
    let url = net::dir_url(&format!("{repo_url}{dir_rel}"));
    let (events, _drain) = mpsc::unbounded_channel::<Event>();
    Nxr::new(net::config(url, auth.clone()), events)?
        .ls_entries()
        .await
        .context("list entries")
}

/// Prints one listing: `name/` for folders, `name` for files.
fn print_entries(indent: &str, entries: &[Entry]) {
    for entry in entries {
        match entry.kind {
            EntryKind::Dir => println!("{indent}  {}/", entry.name),
            EntryKind::File => println!("{indent}  {}", entry.name),
        }
    }
}
