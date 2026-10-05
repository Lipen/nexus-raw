//! Async operations over the [`Nxr`] facade.
//! The bootstrap runs before the TUI opens, the rest reports back through the app channel.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use nexus_raw_core::creds;
use nexus_raw_core::{ArtifactName, Config, EntryKind, Enumeration, Error, Event, Nxr};
use tokio::sync::mpsc;

use crate::app::{DlEv, Msg};
use crate::config::ServerCfg;

/// Resolves the `Authorization` header value from `-u user:pass` and the environment.
/// The fallback order matches the `nxr` CLI: `-u`, then `NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`.
/// One credential set per invocation: it applies to every server of the session.
///
/// # Errors
///
/// Returns an error when `-u` carries no `:` separator (a misuse, without
/// echoing the value) or the credential environment is inconsistent.
pub fn auth_header(user: &Option<String>) -> anyhow::Result<Option<String>> {
    let explicit = user
        .as_deref()
        .map(|raw| -> anyhow::Result<(&str, &str)> {
            match raw.split_once(':') {
                Some((user, pass)) => Ok((user, pass)),
                None => Err(Error::Misuse(
                    "-u expects user:pass with a single ':'; the value carries none".to_owned(),
                )
                .into()),
            }
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

/// The repository list of one server through `service_repos`.
///
/// # Errors
///
/// Returns the core error of the refusing server: the caller decides between
/// the fail-fast shutdown (startup) and a status line (the `s` overlay).
pub async fn server_repos(
    server: &ServerCfg,
    auth: Option<String>,
) -> Result<Vec<nexus_raw_core::service::RepoInfo>, Error> {
    let (events, _drain) = mpsc::unbounded_channel::<Event>();
    Nxr::new(config(dir_url(&server.url), auth), events)?
        .service_repos()
        .await
}

/// Loads the repository list of one server in the background.
/// `slot` is the tab index the result lands in: `tabs.len()` for a server
/// being added, an existing index for a refresh.
pub fn load_repos(
    tx: mpsc::UnboundedSender<Msg>,
    server: ServerCfg,
    slot: usize,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = server_repos(&server, auth).await;
        let _ = tx.send(Msg::Repos {
            tab: slot,
            server,
            res,
        });
    });
}

/// Lists the immediate children of one directory URL through `ls_entries`.
/// `gen` is the generation of the request: a stale result is dropped by the app.
pub fn load_entries(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    dir_url: String,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(dir_url.clone(), auth.clone()), events)?
                .ls_entries()
                .await
        }
        .await;
        let _ = tx.send(Msg::Entries { tab, gen, res });
    });
}

/// HEADs one file URL: the size line of the `i` info action.
/// `name` rides along so the app can label the info line on arrival.
pub fn head_size(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    name: String,
    url: String,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(url.clone(), auth.clone()), events)?
                .head(&url)
                .await
        }
        .await;
        let _ = tx.send(Msg::Head {
            tab,
            gen,
            name,
            res,
        });
    });
}

/// Collects the names behind a download request and reports them to the app.
/// A folder entry walks the whole subtree, a file entry is a single-name plan.
pub fn walk_for_download(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    repo_url: String,
    rel: String,
    kind: EntryKind,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            match kind {
                EntryKind::Dir => collect_dir(&repo_url, &format!("{rel}/"), auth).await,
                EntryKind::File => Ok(vec![ArtifactName::parse(&rel)?]),
            }
        }
        .await;
        let _ = tx.send(Msg::Walk { tab, gen, res });
    });
}

/// Every file under one directory of the repository, names relative to the repo root.
/// The `.sha256` siblings stay out: `ls_entries` never reports them.
///
/// # Errors
///
/// Returns the first listing error of the walk and [`Error::UnsafeName`] when a name fails the grammar.
pub async fn collect_dir(
    repo_url: &str,
    dir_rel: &str,
    auth: Option<String>,
) -> Result<Vec<ArtifactName>, Error> {
    let mut names = Vec::new();
    walk_dir(repo_url, dir_rel, &mut names, auth).await?;
    Ok(names)
}

/// Recursive half of [`collect_dir`]: lists one directory, recurses into folders.
fn walk_dir<'a>(
    repo_url: &'a str,
    dir_rel: &'a str,
    out: &'a mut Vec<ArtifactName>,
    auth: Option<String>,
) -> std::pin::Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
    Box::pin(async move {
        let url = dir_url(&format!("{repo_url}{dir_rel}"));
        let entries = {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            Nxr::new(config(url, auth.clone()), events)?
                .ls_entries()
                .await?
        };
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => {
                    let rel = format!("{dir_rel}{}/", entry.name);
                    walk_dir(repo_url, &rel, out, auth.clone()).await?;
                }
                EntryKind::File => {
                    out.push(ArtifactName::parse(&format!("{dir_rel}{}", entry.name))?);
                }
            }
        }
        Ok(())
    })
}

/// Downloads the collected names from the repository root into `dst`,
/// streaming progress into the app channel.
pub fn start_download(
    tx: mpsc::UnboundedSender<Msg>,
    repo_url: String,
    names: Vec<ArtifactName>,
    dst: PathBuf,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let _ = tx.send(Msg::Dl(DlEv::Start {
            dst: dst.clone(),
            files: names.len(),
        }));
        let (events, mut event_rx) = mpsc::unbounded_channel::<Event>();
        let forward_tx = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = forward_tx.send(fold_event(event));
            }
        });
        let res = async {
            let nxr = Nxr::new(config(repo_url.clone(), auth.clone()), events)?;
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
        Event::ArtifactBytes {
            name, done, total, ..
        } => DlEv::Bytes { name, done, total },
        Event::ArtifactDone { name, skipped, .. } => DlEv::FileDone { name, skipped },
        Event::Retrying {
            name,
            attempt,
            reason,
        } => DlEv::Retry {
            name,
            attempt,
            reason,
        },
        // The summary arrives as the call result; deletion events never fire on `down`.
        Event::Summary(_)
        | Event::Removing { .. }
        | Event::Removed { .. }
        | Event::Missing { .. } => return Msg::Redraw,
    };
    Msg::Dl(dl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_nexus::{MockNexus, Scenario};

    /// The search page the mock serves for every listing query: the whole repo in one page.
    fn seed_three_levels(mock: &MockNexus) {
        for path in [
            "app/core/lib.rs",
            "app/core/util/helpers.py",
            "app/readme.txt",
            "docs/guide/intro.md",
            "README.txt",
        ] {
            mock.insert(path, format!("content of {path}\n").as_bytes());
        }
        mock.insert(
            "service/rest/v1/search/assets",
            br#"{"continuationToken":null,"items":[
                {"path":"app/core/lib.rs"},
                {"path":"app/core/util/helpers.py"},
                {"path":"app/readme.txt"},
                {"path":"docs/guide/intro.md"},
                {"path":"README.txt"}]}"#,
        );
    }

    #[tokio::test]
    async fn server_repos_lists_the_mock_repositories() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        let server = ServerCfg::from_base(mock.base_url());
        let rows = server_repos(&server, None).await.unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["raw-main", "raw-all"]);
        assert!(rows.iter().all(|r| r.format == "raw"));
    }

    #[tokio::test]
    async fn server_repos_refuses_an_unreachable_base() {
        // Port 1 on loopback: connection refused, nothing listens there.
        let server = ServerCfg::from_base("http://127.0.0.1:1/".to_owned());
        let err = server_repos(&server, None).await.unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.hint().is_some());
    }

    #[tokio::test]
    async fn server_repos_refuses_a_malformed_base() {
        let server = ServerCfg::from_base("not a url".to_owned());
        let err = server_repos(&server, None).await.unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[tokio::test]
    async fn collect_dir_walks_the_whole_subtree() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        seed_three_levels(&mock);
        let repo_url = format!("{}repository/raw-main/", mock.base_url());
        let names = collect_dir(&repo_url, "", None).await.unwrap();
        let mut got: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        got.sort_unstable();
        assert_eq!(
            got,
            vec![
                "README.txt",
                "app/core/lib.rs",
                "app/core/util/helpers.py",
                "app/readme.txt",
                "docs/guide/intro.md",
            ]
        );
    }

    #[tokio::test]
    async fn collect_dir_scopes_to_the_named_folder() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        seed_three_levels(&mock);
        let repo_url = format!("{}repository/raw-main/", mock.base_url());
        let names = collect_dir(&repo_url, "app/core/", None).await.unwrap();
        let mut got: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        got.sort_unstable();
        assert_eq!(got, vec!["app/core/lib.rs", "app/core/util/helpers.py"]);
    }

    #[tokio::test]
    async fn collect_dir_on_an_empty_folder_yields_nothing() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        mock.insert(
            "service/rest/v1/search/assets",
            br#"{"continuationToken":null,"items":[{"path":"docs/guide/intro.md"}]}"#,
        );
        let repo_url = format!("{}repository/raw-main/", mock.base_url());
        let names = collect_dir(&repo_url, "docs/", None).await.unwrap();
        let got: Vec<&str> = names.iter().map(|n| n.as_str()).collect();
        assert_eq!(got, vec!["docs/guide/intro.md"]);
        let none = collect_dir(&repo_url, "docs/guide/empty/", None)
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn head_size_reports_the_object_size() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        mock.insert("repository/raw-main/README.txt", b"content of README.txt\n");
        let url = format!("{}repository/raw-main/README.txt", mock.base_url());
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        head_size(tx, 0, 7, "README.txt".into(), url, None);
        match rx.recv().await.unwrap() {
            Msg::Head {
                tab,
                gen,
                name,
                res,
            } => {
                assert_eq!((tab, gen), (0, 7));
                assert_eq!(name, "README.txt");
                let info = res.unwrap();
                assert_eq!(info.size, Some(22));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
