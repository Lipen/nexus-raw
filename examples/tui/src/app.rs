//! TUI state: screens, cursors and the message handlers that mutate them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_raw_core::service::RepoInfo;
use nexus_raw_core::{ArtifactName, Error, Summary};
use tokio::sync::mpsc;

use crate::args::Args;
use crate::net;

/// The three drill-down screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Repositories of every configured server.
    Repos,
    /// Versions of the selected repository.
    Versions,
    /// Objects of the selected version.
    Assets,
}

/// One row of the repository screen.
#[derive(Debug, Clone)]
pub struct RepoRow {
    /// Index into the configured server list.
    pub server: usize,
    /// The service document entry.
    pub repo: RepoInfo,
}

/// Terminal outcome of one download.
pub enum DlOutcome {
    /// The summary of a finished transfer.
    Done(Summary),
    /// The message and hint of a refusal.
    Failed(String, Option<String>),
}

/// Live download state shown in the progress panel.
#[derive(Default)]
pub struct Download {
    /// Local target directory.
    pub dst: PathBuf,
    /// `(to download, skipped)` from the plan event.
    pub planned: Option<(usize, usize)>,
    /// Names currently in flight: `(done, total)` bytes.
    pub active: BTreeMap<String, (u64, Option<u64>)>,
    /// Names finished by this run.
    pub finished: usize,
    /// Names skipped as already complete.
    pub skipped: usize,
    /// The latest retry note, if any.
    pub note: Option<String>,
    /// Set when the transfer ends, one way or the other.
    pub outcome: Option<DlOutcome>,
}

impl Download {
    /// True until the final result arrives.
    pub fn is_running(&self) -> bool {
        self.outcome.is_none()
    }
}

/// Download progress events folded from the core event stream.
pub enum DlEv {
    /// The transfer is set up.
    Start { dst: PathBuf },
    /// The diff plan: `(to download, skipped)`.
    Plan { download: usize, skip: usize },
    /// Coalesced byte progress of one name.
    Bytes { name: String, done: u64, total: Option<u64> },
    /// One name reached its final state.
    FileDone { name: String, skipped: bool },
    /// A request is being retried.
    Retry { name: String, attempt: u32, reason: String },
    /// The call returned.
    Done(Result<Summary, Error>),
}

/// Events the app reacts to: keys, load results and download progress.
pub enum Msg {
    /// A key press.
    Key(KeyEvent),
    /// A redraw request (terminal resize).
    Redraw,
    /// The repository list of one server.
    Repos { server: usize, res: Result<Vec<RepoInfo>, Error> },
    /// The version list of the selected repository.
    Versions { gen: u64, res: Result<Vec<String>, Error> },
    /// The object list of the selected version.
    Assets { gen: u64, res: Result<Vec<ArtifactName>, Error> },
    /// A download progress event.
    Dl(DlEv),
}

/// The whole application state.
pub struct App {
    /// Configured server root URLs.
    pub servers: Vec<String>,
    /// Resolved `Authorization` header value, if any.
    pub auth: Option<String>,
    /// The visible screen.
    pub screen: Screen,
    /// Repository rows across all servers.
    pub repos: Vec<RepoRow>,
    /// Versions of the selected repository.
    pub versions: Vec<String>,
    /// Objects of the selected version.
    pub assets: Vec<ArtifactName>,
    /// Cursor on the repository screen.
    pub repos_cursor: usize,
    /// Cursor on the version screen.
    pub versions_cursor: usize,
    /// Cursor on the object screen.
    pub assets_cursor: usize,
    /// The repository drilled into, if any.
    pub current_repo: Option<RepoRow>,
    /// The version drilled into, if any.
    pub current_version: Option<String>,
    /// The version directory URL, the base of downloads.
    pub current_version_url: Option<String>,
    /// Loads in flight.
    pub pending: usize,
    /// Request generation: results of an older generation are dropped.
    pub gen: u64,
    /// The plain status line.
    pub status: String,
    /// The last error: message plus the core hint.
    pub error: Option<(String, Option<String>)>,
    /// The current or last download, if any.
    pub download: Option<Download>,
    /// Sender back into the message pump.
    pub tx: mpsc::UnboundedSender<Msg>,
    /// Set when the loop should stop.
    pub quit: bool,
}

impl App {
    /// Builds the initial state and resolves credentials.
    ///
    /// # Errors
    ///
    /// Returns an error when `-u` is malformed or the credential environment is inconsistent.
    pub fn new(args: Arc<Args>, tx: mpsc::UnboundedSender<Msg>) -> anyhow::Result<Self> {
        let auth = net::auth_header(&args.user)?;
        let servers = args.bases.clone();
        Ok(Self {
            servers,
            auth,
            screen: Screen::Repos,
            repos: Vec::new(),
            versions: Vec::new(),
            assets: Vec::new(),
            repos_cursor: 0,
            versions_cursor: 0,
            assets_cursor: 0,
            current_repo: None,
            current_version: None,
            current_version_url: None,
            pending: 0,
            gen: 0,
            status: String::new(),
            error: None,
            download: None,
            tx,
            quit: false,
        })
    }

    /// Loads the repository list of every configured server.
    pub fn boot(&mut self) {
        self.status = "loading repositories".into();
        for (index, base) in self.servers.clone().into_iter().enumerate() {
            self.pending += 1;
            net::load_repos(self.tx.clone(), base, index, self.auth.clone());
        }
    }

    /// Handles one message from the pump.
    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Redraw => {}
            Msg::Repos { server, res } => {
                self.pending = self.pending.saturating_sub(1);
                match res {
                    Ok(list) => {
                        let count = list.len();
                        self.repos
                            .extend(list.into_iter().map(|repo| RepoRow { server, repo }));
                        self.status =
                            format!("{count} repositories on {}", self.servers[server]);
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::Versions { gen, res } if gen == self.gen => {
                self.pending = self.pending.saturating_sub(1);
                match res {
                    Ok(versions) if versions.is_empty() => {
                        self.status = "no versions found through the search API".into();
                    }
                    Ok(versions) => {
                        self.versions = versions;
                        self.versions_cursor = 0;
                        self.screen = Screen::Versions;
                        self.status = hint(self.screen);
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::Versions { .. } => {}
            Msg::Assets { gen, res } if gen == self.gen => {
                self.pending = self.pending.saturating_sub(1);
                match res {
                    Ok(assets) if assets.is_empty() => {
                        self.status = "no objects found under this version".into();
                    }
                    Ok(assets) => {
                        self.assets = assets;
                        self.assets_cursor = 0;
                        self.screen = Screen::Assets;
                        self.status = hint(self.screen);
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::Assets { .. } => {}
            Msg::Dl(ev) => self.on_dl(ev),
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.quit = true;
            }
            KeyCode::Char('q') if key.modifiers.is_empty() => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::Enter => self.on_enter(),
            KeyCode::Esc => self.on_back(),
            _ => {}
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let cursor = match self.screen {
            Screen::Repos => &mut self.repos_cursor,
            Screen::Versions => &mut self.versions_cursor,
            Screen::Assets => &mut self.assets_cursor,
        };
        *cursor = ((*cursor as i32) + delta).clamp(0, len as i32 - 1) as usize;
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Repos => self.repos.len(),
            Screen::Versions => self.versions.len(),
            Screen::Assets => self.assets.len(),
        }
    }

    fn on_enter(&mut self) {
        if self.busy() {
            return;
        }
        match self.screen {
            Screen::Repos => self.open_versions(),
            Screen::Versions => self.open_assets(),
            Screen::Assets => self.start_download(),
        }
    }

    /// True while a load or a download is in flight: drills and downloads wait.
    fn busy(&mut self) -> bool {
        if self.pending > 0 {
            self.status = "a listing is in flight".into();
            return true;
        }
        if self.download.as_ref().is_some_and(Download::is_running) {
            self.status = "a download is running".into();
            return true;
        }
        false
    }

    fn open_versions(&mut self) {
        let Some(row) = self.repos.get(self.repos_cursor).cloned() else {
            return;
        };
        self.current_repo = Some(row.clone());
        self.versions.clear();
        self.gen += 1;
        self.pending += 1;
        self.error = None;
        self.status = format!("listing versions of {}", row.repo.name);
        net::load_versions(
            self.tx.clone(),
            net::dir_url(&row.repo.url),
            self.gen,
            self.auth.clone(),
        );
    }

    fn open_assets(&mut self) {
        let Some(repo) = self.current_repo.clone() else {
            return;
        };
        let Some(version) = self.versions.get(self.versions_cursor).cloned() else {
            return;
        };
        let url = net::dir_url(&format!("{}{}/", net::dir_url(&repo.repo.url), version));
        self.current_version = Some(version);
        self.current_version_url = Some(url.clone());
        self.assets.clear();
        self.gen += 1;
        self.pending += 1;
        self.error = None;
        self.status = "listing objects".into();
        net::load_assets(self.tx.clone(), url, self.gen, self.auth.clone());
    }

    fn start_download(&mut self) {
        let Some(repo) = self.current_repo.clone() else {
            return;
        };
        let Some(version) = self.current_version.clone() else {
            return;
        };
        let Some(version_url) = self.current_version_url.clone() else {
            return;
        };
        if self.assets.is_empty() {
            self.status = "nothing to download: the object list is empty".into();
            return;
        }
        let dst = Path::new("nxr-tui-downloads")
            .join(&repo.repo.name)
            .join(&version);
        self.download = Some(Download {
            dst: dst.clone(),
            ..Download::default()
        });
        self.error = None;
        self.status = format!("downloading {}/{} -> {}", repo.repo.name, version, dst.display());
        net::start_download(
            self.tx.clone(),
            version_url,
            self.assets.clone(),
            dst,
            self.auth.clone(),
        );
    }

    fn on_back(&mut self) {
        self.error = None;
        // Listings of an abandoned drill-down are dropped when they arrive.
        self.gen += 1;
        match self.screen {
            Screen::Repos => self.status = "press q to quit".into(),
            Screen::Versions => {
                self.screen = Screen::Repos;
                self.status = hint(self.screen);
            }
            Screen::Assets => {
                self.screen = Screen::Versions;
                self.status = hint(self.screen);
            }
        }
    }

    fn on_dl(&mut self, ev: DlEv) {
        let Some(dl) = self.download.as_mut() else {
            return;
        };
        match ev {
            DlEv::Start { dst } => dl.dst = dst,
            DlEv::Plan { download, skip } => dl.planned = Some((download, skip)),
            DlEv::Bytes { name, done, total } => {
                dl.active.insert(name, (done, total));
            }
            DlEv::FileDone { name, skipped } => {
                dl.active.remove(&name);
                if skipped {
                    dl.skipped += 1;
                } else {
                    dl.finished += 1;
                }
            }
            DlEv::Retry { name, attempt, reason } => {
                dl.note = Some(format!("{name}: retry {attempt}: {reason}"));
            }
            DlEv::Done(Ok(summary)) => {
                dl.outcome = Some(DlOutcome::Done(summary.clone()));
                self.status = format!(
                    "downloaded {}, skipped {}, failed {} -> {}",
                    summary.downloaded,
                    summary.skipped,
                    summary.failed.len(),
                    dl.dst.display()
                );
            }
            DlEv::Done(Err(e)) => {
                let hint = e.hint();
                dl.outcome = Some(DlOutcome::Failed(e.to_string(), hint.clone()));
                self.error = Some((format!("download failed: {e}"), hint));
            }
        }
    }

    fn fail(&mut self, e: Error) {
        self.error = Some((e.to_string(), e.hint()));
    }
}

/// The idle status hint of a screen.
pub fn hint(screen: Screen) -> String {
    match screen {
        Screen::Repos => "enter: versions, up/down: select, q: quit".into(),
        Screen::Versions => "enter: objects, esc: back, up/down: select, q: quit".into(),
        Screen::Assets => "enter: download this version subtree, esc: back, q: quit".into(),
    }
}
