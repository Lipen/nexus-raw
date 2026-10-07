//! Async operations over the [`Nxr`] facade.
//! The bootstrap runs before the TUI opens, the rest reports back through the app channel.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nexus_raw_core::creds;
use nexus_raw_core::model::sibling;
use nexus_raw_core::primitive;
use nexus_raw_core::transport::client::NexusClient;
use nexus_raw_core::{
    ArtifactName, Config, Dir, EntryKind, Enumeration, Error, Event, Nxr, Progress, Summary,
};
use tokio::sync::mpsc;

use crate::app::{DlEv, LocalEntry, Msg};
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
/// being added, an existing index for a refresh. `connect` tags the message
/// so the app releases its single connect slot only for a connect.
pub fn load_repos(
    tx: mpsc::UnboundedSender<Msg>,
    server: ServerCfg,
    slot: usize,
    connect: bool,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = server_repos(&server, auth).await;
        let _ = tx.send(Msg::Repos {
            tab: slot,
            server,
            connect,
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

/// HEADs one file URL for the file card: size and content type.
/// `rel` rides along as the row token: the app drops answers of a card
/// that is no longer open or shows a different path.
pub fn head_size(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    rel: String,
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
        let _ = tx.send(Msg::Head { tab, gen, rel, res });
    });
}

/// Fetches the `.sha256` sibling marker of one file for the file card.
/// `None` when the server has no marker; a malformed marker is an error,
/// because a digest that cannot be read cannot verify anything.
pub fn fetch_sibling(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    rel: String,
    url: String,
    auth: Option<String>,
) {
    tokio::spawn(async move {
        let res = async {
            let (events, _drain) = mpsc::unbounded_channel::<Event>();
            let progress = Progress::new(events);
            let client = NexusClient::new(&config(url.clone(), auth.clone()), progress)?;
            match client.get_small(&url).await? {
                None => Ok(None),
                Some(raw) => sibling::parse_line(&String::from_utf8_lossy(&raw))
                    .map(|sib| Some(sib.digest.as_str().to_owned()))
                    .map_err(|reason| Error::Mismatch {
                        name: url.clone(),
                        detail: format!("the .sha256 marker does not parse: {reason}"),
                    }),
            }
        }
        .await;
        let _ = tx.send(Msg::Sibling { tab, gen, rel, res });
    });
}

/// Downloads one file with the direct `get` primitive into `out` under `dst`,
/// verifying the result against the `.sha256` sibling when the server has one.
/// No enumeration: a single GET, resumable through the `.part` file.
pub fn fetch_one(
    tx: mpsc::UnboundedSender<Msg>,
    url: String,
    out: PathBuf,
    dst: PathBuf,
    auth: Option<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = tx.send(Msg::Dl(DlEv::Start {
            local: dst,
            files: Some(1),
        }));
        let res = fetch_one_inner(url, out, auth).await;
        let _ = tx.send(Msg::Dl(DlEv::Done(res.map(|_size| Summary {
            uploaded: 0,
            downloaded: 1,
            skipped: 0,
            failed: Vec::new(),
            removed: 0,
        }))));
    })
}

async fn fetch_one_inner(url: String, out: PathBuf, auth: Option<String>) -> Result<u64, Error> {
    // The destination folder is explicit (picked with `o` or set in the
    // config), so creating it here is not an implicit side effect.
    // A single-file get cannot fall back to the walk's directory creation.
    if let Some(dir) = out.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::io(dir, e))?;
    }
    let (events, _drain) = mpsc::unbounded_channel::<Event>();
    let progress = Progress::new(events);
    let client = NexusClient::new(&config(url.clone(), auth), progress)?;
    let expected = match client.get_small(&format!("{url}.sha256")).await? {
        None => None,
        Some(raw) => Some(
            sibling::parse_line(&String::from_utf8_lossy(&raw))
                .map_err(|reason| Error::Mismatch {
                    name: url.clone(),
                    detail: format!("the .sha256 marker does not parse: {reason}"),
                })?
                .digest,
        ),
    };
    let got = primitive::get(&client, &url, Some(out), true).await?;
    if let Some(want) = expected {
        let have = got.digest.clone().map(|d| d.as_str().to_owned());
        if have.as_deref() != Some(want.as_str()) {
            return Err(Error::Mismatch {
                name: url,
                detail: format!(
                    "the downloaded digest is {}, the marker pins {}",
                    have.as_deref().unwrap_or("none"),
                    want.as_str()
                ),
            });
        }
    }
    Ok(got.size)
}

/// Lists the children of one folder for the inline expansion.
/// `dir_rel` is the path below the current tree position.
pub fn load_children(
    tx: mpsc::UnboundedSender<Msg>,
    tab: usize,
    gen: u64,
    dir_rel: String,
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
        let _ = tx.send(Msg::Expand {
            tab,
            gen,
            dir_rel,
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
                // The empty scope is the whole repository: no trailing slash,
                // or every name would carry a leading one and fail the grammar.
                EntryKind::Dir if rel.is_empty() => collect_dir(&repo_url, "", auth).await,
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

/// Lists one folder of the local filesystem for the dual-pane, in the
/// background: `read_dir` plus `symlink_metadata` per child.
pub fn local_entries(tx: mpsc::UnboundedSender<Msg>, cwd: PathBuf) {
    tokio::task::spawn_blocking(move || {
        let res = list_local(&cwd);
        let _ = tx.send(Msg::LocalEntries { cwd, res });
    });
}

/// The children of one folder: kinds and file sizes, unsorted.
///
/// # Errors
///
/// Returns [`Error::Io`] when the folder cannot be read or a child cannot be stat-ed.
pub fn list_local(cwd: &Path) -> Result<Vec<LocalEntry>, Error> {
    let mut out = Vec::new();
    let read = std::fs::read_dir(cwd).map_err(|e| Error::io(cwd, e))?;
    for entry in read {
        let entry = entry.map_err(|e| Error::io(cwd, e))?;
        let path = entry.path();
        let meta = path.symlink_metadata().map_err(|e| Error::io(&path, e))?;
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        let size = if kind == EntryKind::File {
            Some(meta.len())
        } else {
            None
        };
        out.push(LocalEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind,
            size,
        });
    }
    Ok(out)
}

/// Runs one core transfer task: announces it, folds its events of the given
/// direction into the app channel and reports the result.
fn spawn_transfer(
    tx: mpsc::UnboundedSender<Msg>,
    start: DlEv,
    dir: Dir,
    task: impl FnOnce(
            mpsc::UnboundedSender<Event>,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Summary, Error>> + Send>,
        > + Send
        + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = tx.send(Msg::Dl(start));
        let (events, mut event_rx) = mpsc::unbounded_channel::<Event>();
        let forward_tx = tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let Some(msg) = fold_event(event, dir) else {
                    continue;
                };
                let _ = forward_tx.send(msg);
            }
        });
        let res = task(events).await;
        let _ = tx.send(Msg::Dl(DlEv::Done(res)));
        // The Nxr is gone, so the event channel is closing: the forwarder drains and exits.
        let _ = forwarder.await;
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
) -> tokio::task::JoinHandle<()> {
    let start = DlEv::Start {
        local: dst.clone(),
        files: Some(names.len()),
    };
    spawn_transfer(tx, start, Dir::Down, |events| {
        Box::pin(async move {
            let nxr = Nxr::new(config(repo_url.clone(), auth.clone()), events)?;
            nxr.down(&dst, Enumeration::Names(names), false).await
        })
    })
}

/// Uploads the local directory `src` into the remote directory `base`,
/// always generating the `.sha256` markers, streaming progress into the app channel.
pub fn start_upload(
    tx: mpsc::UnboundedSender<Msg>,
    base: String,
    src: PathBuf,
    auth: Option<String>,
) -> tokio::task::JoinHandle<()> {
    let start = DlEv::Start {
        local: src.clone(),
        files: None,
    };
    spawn_transfer(tx, start, Dir::Up, |events| {
        Box::pin(async move {
            let nxr = Nxr::new(config(base.clone(), auth.clone()), events)?;
            nxr.up(&src, None, true, None).await
        })
    })
}

/// Folds a core event into a transfer progress message of the given direction.
/// Events of the other direction are dropped: the panel shows one transfer.
fn fold_event(event: Event, dir: Dir) -> Option<Msg> {
    let dl = match event {
        Event::Plan {
            upload,
            download,
            skip,
        } => {
            let (transfer, skip) = match dir {
                Dir::Up => (upload.len(), skip.len()),
                Dir::Down => (download.len(), skip.len()),
            };
            DlEv::Plan { transfer, skip }
        }
        Event::ArtifactStarted {
            name,
            total,
            dir: seen,
        } if seen == dir => DlEv::Bytes {
            name,
            done: 0,
            total,
        },
        Event::ArtifactBytes {
            name,
            done,
            total,
            dir: seen,
        } if seen == dir => DlEv::Bytes { name, done, total },
        Event::ArtifactDone {
            name,
            skipped,
            dir: seen,
            ..
        } if seen == dir => DlEv::FileDone { name, skipped },
        Event::Retrying {
            name,
            attempt,
            reason,
        } => DlEv::Retry {
            name,
            attempt,
            reason,
        },
        // The summary arrives as the call result, deletion events never fire
        // on a transfer, and the other direction belongs to another panel.
        _ => return None,
    };
    Some(Msg::Dl(dl))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mock_nexus::{MockNexus, Scenario};

    use nexus_raw_core::Digest;

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
            Msg::Head { tab, gen, rel, res } => {
                assert_eq!((tab, gen), (0, 7));
                assert_eq!(rel, "README.txt");
                let info = res.unwrap();
                assert_eq!(info.size, Some(22));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_one_downloads_and_verifies_the_marker() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        let body = b"content of README.txt\n".to_vec();
        mock.insert("repository/raw-main/README.txt", &body);
        let digest = Digest::of_bytes(&body);
        let marker = sibling::format_line("README.txt", &digest);
        mock.insert("repository/raw-main/README.txt.sha256", marker.as_bytes());
        let url = format!("{}repository/raw-main/README.txt", mock.base_url());
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("README.txt");
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        fetch_one(tx, url, out.clone(), dir.path().to_path_buf(), None);
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { files, .. }) => assert_eq!(files, Some(1)),
            other => panic!("unexpected message: {other:?}"),
        }
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Done(Ok(summary))) => assert_eq!(summary.downloaded, 1),
            other => panic!("unexpected message: {other:?}"),
        }
        assert_eq!(std::fs::read(&out).unwrap(), body);
    }

    #[tokio::test]
    async fn fetch_one_reports_a_diverging_marker() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        let body = b"content of README.txt\n";
        mock.insert("repository/raw-main/README.txt", body.as_slice());
        // The marker pins somebody else's digest: the transfer must fail loudly.
        let other = Digest::of_bytes(b"not the payload");
        let marker = sibling::format_line("README.txt", &other);
        mock.insert("repository/raw-main/README.txt.sha256", marker.as_bytes());
        let url = format!("{}repository/raw-main/README.txt", mock.base_url());
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("README.txt");
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        fetch_one(tx, url, out.clone(), dir.path().to_path_buf(), None);
        while let Some(msg) = rx.recv().await {
            if let Msg::Dl(DlEv::Done(res)) = msg {
                let err = res.expect_err("the marker mismatch must fail the download");
                assert!(err.to_string().contains("mismatch"), "{err}");
                return;
            }
        }
        panic!("the transfer never finished");
    }

    #[tokio::test]
    async fn fetch_sibling_parses_the_marker_line() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        let body = b"content of README.txt\n".to_vec();
        let digest = Digest::of_bytes(&body);
        let marker = sibling::format_line("README.txt", &digest);
        mock.insert("repository/raw-main/README.txt.sha256", marker.as_bytes());
        let url = format!("{}repository/raw-main/README.txt.sha256", mock.base_url());
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        fetch_sibling(tx, 0, 3, "README.txt".into(), url, None);
        match rx.recv().await.unwrap() {
            Msg::Sibling { tab, gen, rel, res } => {
                assert_eq!((tab, gen), (0, 3));
                assert_eq!(rel, "README.txt");
                assert_eq!(res.unwrap(), Some(digest.as_str().to_owned()));
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn start_upload_puts_bytes_and_the_marker() {
        let mock = MockNexus::start(Scenario::Atomic).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let body = b"payload of the upload\n";
        std::fs::write(dir.path().join("one.txt"), body).unwrap();
        let base = format!("{}repository/raw-main/", mock.base_url());
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        start_upload(tx, base, dir.path().to_path_buf(), None);
        let mut summary = None;
        while let Some(msg) = rx.recv().await {
            if let Msg::Dl(DlEv::Done(res)) = msg {
                summary = Some(res.unwrap());
                break;
            }
        }
        let summary = summary.expect("the upload finished");
        assert_eq!(summary.uploaded, 1);
        assert_eq!(summary.skipped, 0);
        // The object sits in the store byte-for-byte, the marker pins the digest.
        assert_eq!(
            mock.store_get("repository/raw-main/one.txt").as_deref(),
            Some(body.as_slice())
        );
        let marker = mock
            .store_get("repository/raw-main/one.txt.sha256")
            .unwrap();
        let digest = Digest::of_bytes(body);
        assert!(marker
            .windows(digest.as_str().len())
            .any(|w| w == digest.as_str().as_bytes()));
        // A rerun skips the complete object.
        let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
        start_upload(
            tx,
            format!("{}repository/raw-main/", mock.base_url()),
            dir.path().to_path_buf(),
            None,
        );
        let mut summary = None;
        while let Some(msg) = rx.recv().await {
            if let Msg::Dl(DlEv::Done(res)) = msg {
                summary = Some(res.unwrap());
                break;
            }
        }
        assert_eq!(summary.unwrap().skipped, 1, "the rerun skips what landed");
    }

    #[test]
    fn list_local_reports_kinds_and_sizes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("one.bin"), [0u8; 300]).unwrap();
        let mut entries = list_local(dir.path()).unwrap();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "one.bin");
        assert_eq!(entries[0].kind, EntryKind::File);
        assert_eq!(entries[0].size, Some(300));
        assert_eq!(entries[1].name, "sub");
        assert_eq!(entries[1].kind, EntryKind::Dir);
        assert_eq!(entries[1].size, None);
        // An unreadable folder is an io error.
        let err = list_local(&dir.path().join("absent")).unwrap_err();
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn the_up_fold_maps_the_plan_bytes_and_retries() {
        let up = Dir::Up;
        let msg = fold_event(
            Event::Plan {
                upload: vec!["a.rs".into(), "b.rs".into()],
                download: vec![],
                skip: vec!["c.rs".into()],
            },
            up,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::Plan { transfer, skip }) => {
                assert_eq!(transfer, 2, "the plan counts the upload side");
                assert_eq!(skip, 1);
            }
            other => panic!("unexpected message: {other:?}"),
        }
        let msg = fold_event(
            Event::ArtifactStarted {
                name: "a.rs".into(),
                dir: up,
                total: Some(100),
            },
            up,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::Bytes { name, done, total }) => {
                assert_eq!(name, "a.rs");
                assert_eq!(done, 0);
                assert_eq!(total, Some(100));
            }
            other => panic!("unexpected message: {other:?}"),
        }
        let msg = fold_event(
            Event::ArtifactBytes {
                name: "a.rs".into(),
                dir: up,
                done: 40,
                total: Some(100),
            },
            up,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::Bytes { name, done, total }) => {
                assert_eq!(name, "a.rs");
                assert_eq!(done, 40);
                assert_eq!(total, Some(100));
            }
            other => panic!("unexpected message: {other:?}"),
        }
        let msg = fold_event(
            Event::ArtifactDone {
                name: "a.rs".into(),
                dir: up,
                skipped: false,
                done: 100,
                total: Some(100),
            },
            up,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::FileDone { name, skipped }) => {
                assert_eq!(name, "a.rs");
                assert!(!skipped);
            }
            other => panic!("unexpected message: {other:?}"),
        }
        let msg = fold_event(
            Event::Retrying {
                name: "a.rs".into(),
                attempt: 2,
                reason: "reset by peer".into(),
            },
            up,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::Retry {
                name,
                attempt,
                reason,
            }) => {
                assert_eq!(name, "a.rs");
                assert_eq!(attempt, 2);
                assert_eq!(reason, "reset by peer");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn the_fold_drops_the_other_direction_and_the_summary() {
        let up = Dir::Up;
        let down = Dir::Down;
        // An event of the other direction never leaks into this panel.
        assert!(fold_event(
            Event::ArtifactStarted {
                name: "x.rs".into(),
                dir: down,
                total: None,
            },
            up,
        )
        .is_none());
        assert!(fold_event(
            Event::ArtifactDone {
                name: "x.rs".into(),
                dir: up,
                skipped: false,
                done: 1,
                total: None,
            },
            down,
        )
        .is_none());
        // The summary arrives as the call result, never as an event.
        assert!(fold_event(Event::Summary(Summary::default()), up).is_none());
        // The down fold counts the download side of the plan.
        let msg = fold_event(
            Event::Plan {
                upload: vec!["a.rs".into()],
                download: vec!["b.rs".into(), "c.rs".into()],
                skip: vec![],
            },
            down,
        )
        .unwrap();
        match msg {
            Msg::Dl(DlEv::Plan { transfer, skip }) => {
                assert_eq!(transfer, 2, "the plan counts the download side");
                assert_eq!(skip, 0);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
