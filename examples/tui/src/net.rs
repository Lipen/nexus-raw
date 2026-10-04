//! Async operations over the [`Nxr`] facade.
//! Every call owns a short-lived instance and reports back through the app channel.

use std::path::PathBuf;
use std::time::Duration;

use nexus_raw_core::creds;
use nexus_raw_core::{ArtifactName, Config, Enumeration, Event, Nxr};
use tokio::sync::mpsc;

use crate::app::{DlEv, Msg};

/// Resolves the `Authorization` header value from `-u user:pass` and the environment.
/// The fallback order matches the `nxr` CLI: `-u`, then `NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`.
///
/// # Errors
///
/// Returns an error when `-u` carries no `:` separator or the credential environment is inconsistent.
pub fn auth_header(user: &Option<String>) -> anyhow::Result<Option<String>> {
    let explicit = user
        .as_deref()
        .map(|raw| match raw.split_once(':') {
            Some((user, pass)) => Ok((user, pass)),
            None => Err(anyhow::anyhow!("-u expects user:pass, got {raw:?}")),
        })
        .transpose()?;
    Ok(creds::resolve(explicit)?.map(|creds| creds.header))
}

/// A config for one base, with the same defaults as the `nxr` CLI.
#[must_use]
pub fn config(base: String, auth: Option<String>) -> Config {
    Config {
        base,
        tls_insecure: false,
        workers: 8,
        retry_attempts: 4,
        connect_timeout: Duration::from_secs(15),
        stall_timeout: Duration::from_secs(30),
        auth,
    }
}

/// Appends a trailing `/` when missing: every core base is a directory URL.
#[must_use]
pub fn dir_url(url: &str) -> String {
    if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    }
}

/// Lists the repositories of one server root.
pub fn load_repos(
    tx: mpsc::UnboundedSender<Msg>,
    base: String,
    server: usize,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(base.clone(), auth.clone()), events)?
                .service_repos()
                .await
        }
        .await;
        let _ = tx.send(Msg::Repos { server, res });
    });
}

/// Lists the versions of one repository through the search API.
pub fn load_versions(
    tx: mpsc::UnboundedSender<Msg>,
    repo_url: String,
    gen: u64,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(repo_url.clone(), auth.clone()), events)?
                .ls_versions()
                .await
        }
        .await;
        let _ = tx.send(Msg::Versions { gen, res });
    });
}

/// Lists the objects of one version directory through the search API.
pub fn load_assets(
    tx: mpsc::UnboundedSender<Msg>,
    version_url: String,
    gen: u64,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(version_url.clone(), auth.clone()), events)?
                .ls_assets()
                .await
        }
        .await;
        let _ = tx.send(Msg::Assets { gen, res });
    });
}

/// Downloads the version subtree into `dst`, streaming progress into the app channel.
/// The names come from the object listing, so the download matches what the screen shows.
pub fn start_download(
    tx: mpsc::UnboundedSender<Msg>,
    version_url: String,
    names: Vec<ArtifactName>,
    dst: PathBuf,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let _ = tx.send(Msg::Dl(DlEv::Start { dst: dst.clone() }));
        let (events, mut event_rx) = mpsc::unbounded_channel::<Event>();
        let forward_tx = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = forward_tx.send(fold_event(event));
            }
        });
        let res = async {
            let nxr = Nxr::new(config(version_url.clone(), auth.clone()), events)?;
            nxr.down(&dst, Enumeration::Names(names), false).await
        }
        .await;
        let _ = tx.send(Msg::Dl(DlEv::Done(res)));
        // The Nxr is gone, so the event channel is closing: the forwarder drains and exits.
        let _ = forwarder.await;
    });
}

/// Folds a core event into a download progress message.
fn fold_event(event: Event) -> Msg {
    let dl = match event {
        Event::Plan { download, skip, .. } => DlEv::Plan {
            download: download.len(),
            skip: skip.len(),
        },
        Event::ArtifactStarted { name, total, .. } => DlEv::Bytes {
            name,
            done: 0,
            total,
        },
        Event::ArtifactBytes { name, done, total, .. } => DlEv::Bytes { name, done, total },
        Event::ArtifactDone { name, skipped, .. } => DlEv::FileDone { name, skipped },
        Event::Retrying { name, attempt, reason } => DlEv::Retry { name, attempt, reason },
        // The summary arrives as the call result; deletion events never fire on `down`.
        Event::Summary(_)
        | Event::Removing { .. }
        | Event::Removed { .. }
        | Event::Missing { .. } => return Msg::Redraw,
    };
    Msg::Dl(dl)
}
