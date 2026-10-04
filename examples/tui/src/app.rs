//! TUI state: screens, cursors and the message handlers that mutate them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_raw_core::service::RepoInfo;
use nexus_raw_core::{ArtifactName, Entry, EntryKind, Error, Summary};
use tokio::sync::mpsc;

use crate::args::Args;
use crate::net;

/// The two screens: the server repositories, then the tree of the selected repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Repositories of every configured server.
    Repos,
    /// The tree of the selected repository, descended as deep as it goes.
    Tree,
}

/// One row of the repository screen.
#[derive(Debug, Clone)]
pub struct RepoRow {
    /// Index into the configured server list.
    pub server: usize,
    /// The service document entry.
    pub repo: RepoInfo,
}

impl RepoRow {
    /// Whether this repository can be opened: raw, unless `--all-formats` lifted the filter.
    pub fn enterable(&self, all_formats: bool) -> bool {
        all_formats || self.repo.format == "raw"
    }
}

/// Terminal outcome of one download.
#[derive(Debug)]
pub enum DlOutcome {
    /// The summary of a finished transfer.
    Done(Summary),
    /// The message and hint of a refusal.
    Failed(String, Option<String>),
}

/// Live download state shown in the progress panel.
#[derive(Debug, Default)]
pub struct Download {
    /// The repository URL the names are relative to.
    pub repo_url: String,
    /// Local target directory.
    pub dst: PathBuf,
    /// The plan count of the walk, shown before the transfer starts.
    pub files: Option<usize>,
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
#[derive(Debug)]
pub enum DlEv {
    /// The transfer is set up: destination and the plan count of the walk.
    Start { dst: PathBuf, files: usize },
    /// The diff plan: `(to download, skipped)`.
    Plan { download: usize, skip: usize },
    /// Coalesced byte progress of one name.
    Bytes {
        name: String,
        done: u64,
        total: Option<u64>,
    },
    /// One name reached its final state.
    FileDone { name: String, skipped: bool },
    /// A request is being retried.
    Retry {
        name: String,
        attempt: u32,
        reason: String,
    },
    /// The call returned.
    Done(Result<Summary, Error>),
}

/// Events the app reacts to: keys, listing results and download progress.
#[derive(Debug)]
pub enum Msg {
    /// A key press.
    Key(KeyEvent),
    /// A redraw request (terminal resize).
    Redraw,
    /// The child list of one tree directory.
    Entries {
        gen: u64,
        res: Result<Vec<Entry>, Error>,
    },
    /// The file walk behind a download request.
    Walk {
        gen: u64,
        res: Result<Vec<ArtifactName>, Error>,
    },
    /// A download progress event.
    Dl(DlEv),
}

/// The whole application state.
pub struct App {
    /// Configured server root URLs.
    pub servers: Vec<String>,
    /// Resolved `Authorization` header value, if any.
    pub auth: Option<String>,
    /// `--all-formats`: repositories of every format may be opened.
    pub all_formats: bool,
    /// The visible screen.
    pub screen: Screen,
    /// Repository rows across all servers, loaded before the TUI opened.
    pub repos: Vec<RepoRow>,
    /// Cursor on the repository screen.
    pub repos_cursor: usize,
    /// The repository drilled into, if any.
    pub current_repo: Option<RepoRow>,
    /// Path segments below the repository root, one per descended folder.
    pub path: Vec<String>,
    /// The child list of the current tree position.
    pub entries: Vec<Entry>,
    /// Cursor on the tree screen.
    pub tree_cursor: usize,
    /// Loads and walks in flight.
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
    /// Builds the state over the bootstrapped repository list.
    #[must_use]
    pub fn new(
        args: Arc<Args>,
        repos: Vec<RepoRow>,
        auth: Option<String>,
        tx: mpsc::UnboundedSender<Msg>,
    ) -> Self {
        Self {
            servers: args.bases.clone(),
            auth,
            all_formats: args.all_formats,
            screen: Screen::Repos,
            repos,
            repos_cursor: 0,
            current_repo: None,
            path: Vec::new(),
            entries: Vec::new(),
            tree_cursor: 0,
            pending: 0,
            gen: 0,
            status: String::new(),
            error: None,
            download: None,
            tx,
            quit: false,
        }
    }

    /// Fills the first status line: the screen is already populated, nothing is in flight.
    pub fn boot(&mut self) {
        self.status = format!(
            "{} repositories on {} server(s), enter opens a raw repository",
            self.repos.len(),
            self.servers.len()
        );
    }

    /// Handles one message from the pump.
    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Redraw => {}
            Msg::Entries { gen, res } if gen == self.gen => {
                self.pending = self.pending.saturating_sub(1);
                match res {
                    Ok(entries) => {
                        self.entries = entries;
                        self.tree_cursor = 0;
                        self.screen = Screen::Tree;
                        self.status = if self.entries.is_empty() {
                            "the folder is empty, esc goes back up".into()
                        } else {
                            hint(self.screen)
                        };
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::Entries { .. } => {}
            Msg::Walk { gen, res } if gen == self.gen => {
                self.pending = self.pending.saturating_sub(1);
                match res {
                    Ok(names) if names.is_empty() => {
                        self.status = "the subtree holds no files".into();
                    }
                    Ok(names) => {
                        let count = names.len();
                        let unit = if count == 1 { "file" } else { "files" };
                        let Some(dl) = self.download.as_ref() else {
                            return;
                        };
                        let dst = dl.dst.clone();
                        let repo_url = dl.repo_url.clone();
                        self.status = format!("plan: {count} {unit} -> {}", dst.display());
                        net::start_download(
                            self.tx.clone(),
                            repo_url,
                            names,
                            dst,
                            self.auth.clone(),
                        );
                    }
                    Err(e) => self.fail(e),
                }
            }
            Msg::Walk { .. } => {}
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
            KeyCode::Esc | KeyCode::Backspace => self.on_back(),
            KeyCode::Char('d') if key.modifiers.is_empty() => self.request_download(),
            _ => {}
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = match self.screen {
            Screen::Repos => self.repos.len(),
            Screen::Tree => self.entries.len(),
        };
        if len == 0 {
            return;
        }
        let cursor = match self.screen {
            Screen::Repos => &mut self.repos_cursor,
            Screen::Tree => &mut self.tree_cursor,
        };
        *cursor = ((*cursor as i32) + delta).clamp(0, len as i32 - 1) as usize;
    }

    fn on_enter(&mut self) {
        if self.busy() {
            return;
        }
        match self.screen {
            Screen::Repos => self.open_repo(),
            Screen::Tree => {
                let Some(entry) = self.entries.get(self.tree_cursor) else {
                    return;
                };
                match entry.kind {
                    EntryKind::Dir => {
                        let name = entry.name.clone();
                        self.descend(name);
                    }
                    EntryKind::File => self.request_download(),
                }
            }
        }
    }

    /// True while a load, a walk or a download is in flight: drills and downloads wait.
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

    fn open_repo(&mut self) {
        let Some(row) = self.repos.get(self.repos_cursor).cloned() else {
            return;
        };
        if !row.enterable(self.all_formats) {
            self.status = format!(
                "format {}: not enterable, --all-formats lifts the filter",
                row.repo.format
            );
            return;
        }
        self.current_repo = Some(row.clone());
        self.path.clear();
        self.entries.clear();
        self.gen += 1;
        self.pending += 1;
        self.error = None;
        self.status = format!("listing {}/", row.repo.name);
        net::load_entries(
            self.tx.clone(),
            net::dir_url(&row.repo.url),
            self.gen,
            self.auth.clone(),
        );
    }

    fn descend(&mut self, name: String) {
        if self.current_repo.is_none() {
            return;
        }
        self.status = format!("listing {name}/");
        self.path.push(name);
        let url = self.here_url();
        self.entries.clear();
        self.gen += 1;
        self.pending += 1;
        self.error = None;
        net::load_entries(self.tx.clone(), url, self.gen, self.auth.clone());
    }

    /// The directory URL of the current tree position: the repo root plus the descended path.
    #[must_use]
    pub fn here_url(&self) -> String {
        let Some(repo) = &self.current_repo else {
            return String::new();
        };
        let base = net::dir_url(&repo.repo.url);
        if self.path.is_empty() {
            base
        } else {
            net::dir_url(&format!("{base}{}/", self.path.join("/")))
        }
    }

    /// The path from the repo root for the header: `raw-main / app / core`.
    #[must_use]
    pub fn breadcrumb(&self) -> String {
        let mut segs = Vec::with_capacity(1 + self.path.len());
        if let Some(repo) = &self.current_repo {
            segs.push(repo.repo.name.clone());
        }
        segs.extend(self.path.iter().cloned());
        segs.join(" / ")
    }

    /// The download action on the selected tree entry:
    /// a folder walks and downloads its whole subtree, a file downloads itself.
    fn request_download(&mut self) {
        let Some(repo) = self.current_repo.clone() else {
            return;
        };
        let Some(entry) = self.entries.get(self.tree_cursor) else {
            return;
        };
        let rel = if self.path.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", self.path.join("/"), entry.name)
        };
        let dst = Path::new("nxr-tui-downloads").join(&repo.repo.name);
        self.download = Some(Download {
            repo_url: net::dir_url(&repo.repo.url),
            dst: dst.clone(),
            ..Download::default()
        });
        self.error = None;
        self.gen += 1;
        self.pending += 1;
        self.status = match entry.kind {
            EntryKind::Dir => format!("walking {rel}/"),
            EntryKind::File => format!("plan: 1 file -> {}", dst.display()),
        };
        net::walk_for_download(
            self.tx.clone(),
            net::dir_url(&repo.repo.url),
            rel,
            entry.kind,
            self.gen,
            self.auth.clone(),
        );
    }

    fn on_back(&mut self) {
        self.error = None;
        // Listings of an abandoned drill-down are dropped when they arrive.
        self.gen += 1;
        match self.screen {
            Screen::Repos => self.status = "press q to quit".into(),
            Screen::Tree if self.path.is_empty() => {
                self.screen = Screen::Repos;
                self.entries.clear();
                self.status = hint(self.screen);
            }
            Screen::Tree => {
                self.path.pop();
                let url = self.here_url();
                self.entries.clear();
                self.pending += 1;
                self.status = format!("listing {}/", self.breadcrumb());
                net::load_entries(self.tx.clone(), url, self.gen, self.auth.clone());
            }
        }
    }

    fn on_dl(&mut self, ev: DlEv) {
        let Some(dl) = self.download.as_mut() else {
            return;
        };
        match ev {
            DlEv::Start { dst, files } => {
                dl.dst = dst;
                dl.files = Some(files);
            }
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
            DlEv::Retry {
                name,
                attempt,
                reason,
            } => {
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
        Screen::Repos => "enter: open the selected repository, up/down: select, q: quit".into(),
        Screen::Tree => {
            "enter: open folder or download file, d: download entry, esc/backspace: up, q: quit"
                .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_raw_core::service::RepoInfo;

    fn row(name: &str, format: &str, kind: &str) -> RepoRow {
        RepoRow {
            server: 0,
            repo: RepoInfo {
                name: name.to_owned(),
                format: format.to_owned(),
                kind: kind.to_owned(),
                url: format!("http://127.0.0.1:1/repository/{name}/"),
            },
        }
    }

    fn app_with(repos: Vec<RepoRow>, all_formats: bool) -> (App, mpsc::UnboundedReceiver<Msg>) {
        let args = Arc::new(Args {
            bases: vec!["http://127.0.0.1:1/".to_owned()],
            user: None,
            all_formats,
            smoke: false,
        });
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = App::new(args, repos, None, tx);
        app.boot();
        (app, rx)
    }

    #[test]
    fn only_raw_is_enterable_by_default() {
        let raw = row("raw-main", "raw", "hosted");
        let maven = row("maven-central", "maven2", "proxy");
        assert!(raw.enterable(false));
        assert!(!maven.enterable(false));
        assert!(maven.enterable(true));
        assert!(raw.enterable(true));
    }

    #[tokio::test]
    async fn enter_on_a_non_raw_repo_only_reports() {
        let (mut app, mut rx) = app_with(vec![row("maven-central", "maven2", "proxy")], false);
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.screen, Screen::Repos);
        assert!(app.status.contains("maven2"));
        assert!(app.status.contains("--all-formats"));
        assert!(rx.try_recv().is_err(), "no load was spawned");
    }

    #[tokio::test]
    async fn enter_on_a_non_raw_repo_opens_under_all_formats() {
        let (mut app, mut rx) = app_with(vec![row("maven-central", "maven2", "proxy")], true);
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(
            app.current_repo.as_ref().map(|r| r.repo.name.as_str()),
            Some("maven-central")
        );
        match rx.recv().await.unwrap() {
            Msg::Entries { res, .. } => assert!(res.is_err(), "the mock port refuses"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn enter_on_a_raw_repo_lists_the_tree() {
        let (mut app, mut rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.pending, 1);
        match rx.recv().await.unwrap() {
            Msg::Entries { res, .. } => assert!(res.is_err(), "the mock port refuses"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tree_navigation_descends_and_ascends() {
        let (mut app, _rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.current_repo = app.repos.first().cloned();
        app.handle(Msg::Entries {
            gen: app.gen,
            res: Ok(vec![
                Entry {
                    name: "app".into(),
                    kind: EntryKind::Dir,
                },
                Entry {
                    name: "README.txt".into(),
                    kind: EntryKind::File,
                },
            ]),
        });
        assert_eq!(app.screen, Screen::Tree);
        assert_eq!(app.breadcrumb(), "raw-main");

        // Enter on the first entry: the folder opens and the breadcrumb grows.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.path, vec!["app".to_owned()]);
        assert_eq!(app.breadcrumb(), "raw-main / app");
        assert_eq!(app.pending, 1);
        assert_eq!(
            app.here_url(),
            "http://127.0.0.1:1/repository/raw-main/app/"
        );

        // The child listing lands: `core/` with a file inside.
        app.handle(Msg::Entries {
            gen: app.gen,
            res: Ok(vec![Entry {
                name: "core".into(),
                kind: EntryKind::Dir,
            }]),
        });
        assert_eq!(app.tree_cursor, 0);

        // Backspace climbs one level and reloads the parent listing.
        app.handle(Msg::Key(key(KeyCode::Backspace)));
        assert!(app.path.is_empty());
        assert_eq!(app.pending, 1);
        assert_eq!(app.here_url(), "http://127.0.0.1:1/repository/raw-main/");

        // Esc on the repo root returns to the repositories.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.screen, Screen::Repos);
    }

    #[tokio::test]
    async fn enter_on_a_file_downloads_it_directly() {
        let (mut app, mut rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.current_repo = app.repos.first().cloned();
        app.handle(Msg::Entries {
            gen: app.gen,
            res: Ok(vec![Entry {
                name: "README.txt".into(),
                kind: EntryKind::File,
            }]),
        });
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.status, "plan: 1 file -> nxr-tui-downloads/raw-main");
        match rx.recv().await.unwrap() {
            Msg::Walk { res, .. } => {
                let names = res.unwrap();
                assert_eq!(names.len(), 1);
                assert_eq!(names[0].as_str(), "README.txt");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_on_a_folder_walks_the_subtree() {
        let (mut app, mut rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.current_repo = app.repos.first().cloned();
        app.handle(Msg::Entries {
            gen: app.gen,
            res: Ok(vec![Entry {
                name: "app".into(),
                kind: EntryKind::Dir,
            }]),
        });
        // `d` on the folder entry: the walk starts, the plan count comes with Start.
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert_eq!(app.status, "walking app/");
        match rx.recv().await.unwrap() {
            Msg::Walk { res, .. } => {
                // The mock port refuses: the walk reports the transport error.
                assert!(res.is_err());
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_plan_starts_the_transfer() {
        let (mut app, mut rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.current_repo = app.repos.first().cloned();
        app.handle(Msg::Entries {
            gen: app.gen,
            res: Ok(vec![Entry {
                name: "README.txt".into(),
                kind: EntryKind::File,
            }]),
        });
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let Msg::Walk { gen, res } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        app.handle(Msg::Walk { gen, res });
        assert_eq!(app.status, "plan: 1 file -> nxr-tui-downloads/raw-main");
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { files, .. }) => assert_eq!(files, 1),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stale_listings_are_dropped() {
        let (mut app, mut rx) = app_with(vec![row("raw-main", "raw", "hosted")], false);
        app.handle(Msg::Key(key(KeyCode::Enter)));
        // The user backs out before the listing lands: the generation moved on.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        let Msg::Entries { gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the listing");
        };
        assert_ne!(gen, app.gen, "the stale result must be dropped");
        app.handle(Msg::Entries {
            gen,
            res: Ok(vec![Entry {
                name: "app".into(),
                kind: EntryKind::Dir,
            }]),
        });
        assert_eq!(
            app.screen,
            Screen::Repos,
            "the stale result never opens a screen"
        );
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }
}
