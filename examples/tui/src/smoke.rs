//! `--smoke`: the TUI flow without the terminal.
//! Lists repositories, versions and objects, then downloads the first version subtree and prints the summary.

use std::path::Path;

use anyhow::{bail, Context};
use nexus_raw_core::service::RepoInfo;
use nexus_raw_core::{Enumeration, Event, Nxr};
use tokio::sync::mpsc;

use crate::args::Args;
use crate::net;

/// Runs the headless browse-and-download flow.
///
/// # Errors
///
/// Returns an error when any listing or the download fails.
pub async fn run(args: &Args) -> anyhow::Result<()> {
    let auth = net::auth_header(&args.user)?;
    let mut picked: Option<RepoInfo> = None;
    for base in &args.bases {
        let (events, _drain) = mpsc::unbounded_channel::<Event>();
        let nxr = Nxr::new(net::config(base.clone(), auth.clone()), events)?;
        let repos = nxr.service_repos().await.context("service repos")?;
        println!("server {base}");
        for repo in &repos {
            println!("  {:<12} {:<7} {}  {}", repo.name, repo.format, repo.kind, repo.url);
        }
        if picked.is_none() {
            // A hosted raw repository accepts uploads and holds objects: the download target of choice.
            picked = repos
                .iter()
                .find(|r| r.format == "raw" && r.kind == "hosted")
                .or_else(|| repos.iter().find(|r| r.format == "raw"))
                .cloned();
        }
    }
    let Some(repo) = picked else {
        bail!("no raw repository found on any server");
    };

    let repo_url = net::dir_url(&repo.url);
    let versions = {
        let (events, _drain) = mpsc::unbounded_channel::<Event>();
        let nxr = Nxr::new(net::config(repo_url.clone(), auth.clone()), events)?;
        nxr.ls_versions().await.context("list versions")?
    };
    println!("versions of {}:", repo.name);
    for version in &versions {
        println!("  {version}");
    }
    let Some(version) = versions.first().cloned() else {
        bail!("no versions under {}", repo.name);
    };

    let version_url = net::dir_url(&format!("{repo_url}{version}/"));
    let assets = {
        let (events, _drain) = mpsc::unbounded_channel::<Event>();
        let nxr = Nxr::new(net::config(version_url.clone(), auth.clone()), events)?;
        nxr.ls_assets().await.context("list assets")?
    };
    println!("objects of {version}:");
    for name in &assets {
        println!("  {name}");
    }
    if assets.is_empty() {
        bail!("no objects under {version}");
    }

    let dst = Path::new("nxr-tui-smoke").join(&repo.name).join(&version);
    // A rerun downloads for real instead of skipping everything already present.
    let _ = std::fs::remove_dir_all(&dst);
    println!("downloading {} -> {}", version_url, dst.display());

    let (events, mut event_rx) = mpsc::unbounded_channel::<Event>();
    let printer = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                Event::Plan { download, skip, .. } => {
                    println!("plan: {} to download, {} skipped", download.len(), skip.len());
                }
                Event::ArtifactDone { name, skipped, .. } => {
                    let mark = if skipped { "skip" } else { "done" };
                    println!("  {mark} {name}");
                }
                Event::Retrying { name, attempt, reason } => {
                    println!("  retry {attempt} {name}: {reason}");
                }
                Event::Summary(_) | Event::ArtifactStarted { .. } | Event::ArtifactBytes { .. } => {}
                Event::Removing { .. } | Event::Removed { .. } | Event::Missing { .. } => {}
            }
        }
    });

    let summary = {
        let nxr = Nxr::new(net::config(version_url.clone(), auth.clone()), events)?;
        nxr.down(&dst, Enumeration::Names(assets), false)
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
    println!("smoke ok: {} complete under {}", summary.downloaded, dst.display());
    Ok(())
}
