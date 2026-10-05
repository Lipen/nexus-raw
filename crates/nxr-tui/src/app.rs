//! TUI state: tabs, screens, overlays and the message handlers that mutate them.
//!
//! One [`Tab`] per server holds its own screen, cursor and generation counter,
//! so listings of different servers never cancel each other. The app folds
//! [`Msg`] values: key presses, mouse events, listing results and download
//! progress. Rendering lives in [`crate::ui`], the terminal loop in [`crate::tui`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use nexus_raw_core::service::RepoInfo;
use nexus_raw_core::{ArtifactName, Entry, EntryKind, Error, HeadInfo, Summary};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

use crate::config::{self, ConfigFile, Nav, ServerCfg};
use crate::net;

/// A double click is two clicks on the same row within this window.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// The two screens of one tab: the server repositories, then the tree of the
/// selected repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Repositories of the tab's server.
    Repos,
    /// The tree of the selected repository, descended as deep as it goes.
    Tree,
}

/// One server tab: the repository list, the opened repository and its tree.
pub struct Tab {
    /// The server this tab browses: label and root URL.
    pub server: ServerCfg,
    /// The visible screen.
    pub screen: Screen,
    /// The repository list of the server, from `service_repos`.
    pub repos: Vec<RepoInfo>,
    /// Cursor on the repository screen.
    pub repos_cursor: usize,
    /// The repository drilled into, if any.
    pub repo: Option<RepoInfo>,
    /// Path segments below the repository root, one per descended folder.
    pub path: Vec<String>,
    /// The child list of the current tree position.
    pub entries: Vec<Entry>,
    /// Cached children of expanded folders, keyed by their path below the
    /// current tree position.
    pub expanded: BTreeMap<String, Vec<Entry>>,
    /// The folders currently expanded inline.
    pub open: BTreeSet<String>,
    /// Expansion loads in flight, keyed by folder path.
    pub loading_dirs: BTreeSet<String>,
    /// Cursor on the tree screen, indexing the filtered list.
    pub tree_cursor: usize,
    /// The live filter, set with `/`. `Some("")` filters nothing but stays active.
    pub filter: Option<String>,
    /// Request generation of this tab: results of an older generation are dropped.
    pub gen: u64,
    /// True while a listing of the current position is in flight.
    pub loading: bool,
}

/// One row of the expandable tree: an entry of the current listing, or of an
/// expanded folder's cached children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The entry name.
    pub name: String,
    /// The path below the current tree position (`app`, `app/core`).
    pub rel: String,
    /// Folder or file.
    pub kind: EntryKind,
    /// Indentation depth: 0 for the current listing.
    pub depth: usize,
    /// Whether this folder is expanded inline.
    pub expanded: bool,
    /// Whether this folder's children are loading.
    pub loading: bool,
}

impl Tab {
    /// A fresh tab over a bootstrapped repository list.
    #[must_use]
    pub fn new(server: ServerCfg, repos: Vec<RepoInfo>) -> Self {
        Self {
            server,
            screen: Screen::Repos,
            repos,
            repos_cursor: 0,
            repo: None,
            path: Vec::new(),
            entries: Vec::new(),
            expanded: BTreeMap::new(),
            open: BTreeSet::new(),
            loading_dirs: BTreeSet::new(),
            tree_cursor: 0,
            filter: None,
            gen: 0,
            loading: false,
        }
    }

    /// Drops the inline expansion state: the tree folds back to the current listing.
    pub fn clear_expansion(&mut self) {
        self.expanded.clear();
        self.open.clear();
        self.loading_dirs.clear();
    }

    /// The flattened tree rows: depth 0 is the current listing, deeper rows
    /// come from expanded folders' cached children, in listing order.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        self.collect_rows("", 0, &mut out);
        out
    }

    fn collect_rows(&self, dir_rel: &str, depth: usize, out: &mut Vec<Row>) {
        let children = if dir_rel.is_empty() {
            &self.entries
        } else {
            match self.expanded.get(dir_rel) {
                Some(children) => children,
                None => return,
            }
        };
        for entry in children {
            let rel = if dir_rel.is_empty() {
                entry.name.clone()
            } else {
                format!("{dir_rel}/{}", entry.name)
            };
            let expanded = entry.kind == EntryKind::Dir && self.open.contains(&rel);
            let loading = entry.kind == EntryKind::Dir && self.loading_dirs.contains(&rel);
            let row = Row {
                name: entry.name.clone(),
                rel: rel.clone(),
                kind: entry.kind,
                depth,
                expanded,
                loading,
            };
            let expand_into = expanded;
            out.push(row);
            if expand_into {
                self.collect_rows(&rel, depth + 1, out);
            }
        }
    }

    /// The indices of the rows the filter lets through, in listing order.
    #[must_use]
    pub fn row_idx(&self) -> Vec<usize> {
        match &self.filter {
            None => (0..self.rows().len()).collect(),
            Some(f) => {
                let needle = f.to_lowercase();
                self.rows()
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.name.to_lowercase().contains(&needle))
                    .map(|(i, _)| i)
                    .collect()
            }
        }
    }

    /// The selected tree row, through the filter.
    #[must_use]
    pub fn selected_row(&self) -> Option<Row> {
        let idx = self.row_idx();
        let at = *idx.get(self.tree_cursor)?;
        self.rows().into_iter().nth(at)
    }

    /// Whether this repository can be opened: raw, unless the filter was lifted.
    #[must_use]
    pub fn enterable(&self, repo: &RepoInfo, all_formats: bool) -> bool {
        all_formats || repo.format == "raw"
    }

    /// The length of the visible list of the active screen.
    #[must_use]
    pub fn visible_len(&self) -> usize {
        match self.screen {
            Screen::Repos => self.repos.len(),
            Screen::Tree => self.row_idx().len(),
        }
    }

    /// The directory URL of the current tree position: the repo root plus the descended path.
    #[must_use]
    pub fn here_url(&self) -> String {
        let Some(repo) = &self.repo else {
            return String::new();
        };
        let base = net::dir_url(&repo.url);
        if self.path.is_empty() {
            base
        } else {
            net::dir_url(&format!("{base}{}/", self.path.join("/")))
        }
    }

    /// The path from the repo root for the title: `raw-main / app / core`.
    #[must_use]
    pub fn breadcrumb(&self) -> String {
        let mut segs = Vec::with_capacity(1 + self.path.len());
        if let Some(repo) = &self.repo {
            segs.push(repo.name.clone());
        }
        segs.extend(self.path.iter().cloned());
        segs.join(" / ")
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
    #[must_use]
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

/// The modal overlays on top of the normal screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Browsing: repositories and trees.
    Normal,
    /// The server overlay: switch to a known server or add a new one.
    Servers,
    /// The add-server form: the buffer holds the URL typed so far.
    AddServer(String),
    /// The keybindings overlay.
    Help,
}

/// Events the app reacts to: input, listing results and download progress.
#[derive(Debug)]
pub enum Msg {
    /// A key press.
    Key(KeyEvent),
    /// A mouse event (wheel, click).
    Mouse(MouseEvent),
    /// Bracketed paste (URLs into the add-server form).
    Paste(String),
    /// A redraw request (terminal resize).
    Redraw,
    /// The repository list of one server: `tab` is `tabs.len()` for a server
    /// being added, an existing tab index for a refresh. `connect` says which:
    /// only a connect holds the single connect slot.
    Repos {
        tab: usize,
        server: ServerCfg,
        connect: bool,
        res: Result<Vec<RepoInfo>, Error>,
    },
    /// The child list of one tree directory.
    Entries {
        tab: usize,
        gen: u64,
        res: Result<Vec<Entry>, Error>,
    },
    /// The children of one folder expanded inline.
    Expand {
        tab: usize,
        gen: u64,
        dir_rel: String,
        res: Result<Vec<Entry>, Error>,
    },
    /// The file walk behind a download request.
    Walk {
        tab: usize,
        gen: u64,
        res: Result<Vec<ArtifactName>, Error>,
    },
    /// The HEAD result behind the `i` info action.
    Head {
        tab: usize,
        gen: u64,
        name: String,
        res: Result<HeadInfo, Error>,
    },
    /// A download progress event.
    Dl(DlEv),
}

/// The whole application state.
pub struct App {
    /// Every server the session knows: opened tabs, presets and added servers.
    pub servers: Vec<ServerCfg>,
    /// Resolved `Authorization` header value, if any.
    pub auth: Option<String>,
    /// Repositories of every format may be opened, not only raw.
    pub all_formats: bool,
    /// The left/right navigation mode of the tree.
    pub nav: Nav,
    /// Where downloads land, from the config.
    pub download_dir: PathBuf,
    /// The live config: presets added in the `s` overlay are saved here.
    pub config: ConfigFile,
    /// The config path, when one could be resolved.
    pub config_path: Option<PathBuf>,
    /// One tab per open server.
    pub tabs: Vec<Tab>,
    /// The active tab.
    pub tab: usize,
    /// The active overlay.
    pub mode: Mode,
    /// Cursor of the servers overlay.
    pub servers_cursor: usize,
    /// Server connects in flight: one at a time, the slot collides otherwise.
    pub connecting: usize,
    /// Loads, walks and HEADs in flight, across every tab: a diagnostic count,
    /// not a gate (the gates are the per-tab `loading` and the running download).
    pub pending: usize,
    /// The current or last download, if any.
    pub download: Option<Download>,
    /// The plain status line.
    pub status: String,
    /// The last error: message plus the core hint.
    pub error: Option<(String, Option<String>)>,
    /// The `i` info line of the selected tree entry.
    pub info: Option<String>,
    /// Set when the loop should stop.
    pub quit: bool,
    /// Set after the first `q` while a download is running: the second `q` quits.
    pub quit_armed: bool,
    /// The render state of the list: kept across frames so the scroll offset
    /// survives and mouse clicks can hit-test.
    pub list_state: ListState,
    /// The inner area of the list block, set by every frame of [`crate::ui::draw`].
    pub list_area: Rect,
    /// The previous left click: `(when, row index)` for the double click.
    last_click: Option<(Instant, usize)>,
    /// Sender back into the message pump.
    pub tx: mpsc::UnboundedSender<Msg>,
}

impl App {
    /// Builds the state over the bootstrapped tabs.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        servers: Vec<ServerCfg>,
        tabs: Vec<Tab>,
        config: ConfigFile,
        config_path: Option<PathBuf>,
        all_formats: bool,
        download_dir: PathBuf,
        auth: Option<String>,
        tx: mpsc::UnboundedSender<Msg>,
    ) -> Self {
        let nav = config.tui.nav;
        Self {
            servers,
            auth,
            all_formats,
            nav,
            download_dir,
            config,
            config_path,
            tabs,
            tab: 0,
            mode: Mode::Normal,
            servers_cursor: 0,
            connecting: 0,
            pending: 0,
            download: None,
            status: String::new(),
            error: None,
            info: None,
            quit: false,
            quit_armed: false,
            list_state: ListState::default(),
            list_area: Rect::default(),
            last_click: None,
            tx,
        }
    }

    /// Fills the first status line: the screens are already populated, nothing is in flight.
    pub fn boot(&mut self) {
        let repos: usize = self.tabs.iter().map(|t| t.repos.len()).sum();
        self.status = format!(
            "{repos} repositories on {} server(s), enter opens a raw repository",
            self.tabs.len()
        );
    }

    /// Handles one message from the pump.
    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Key(key) => self.on_key(key),
            Msg::Mouse(mouse) => self.on_mouse(mouse),
            Msg::Paste(text) => self.on_paste(&text),
            Msg::Redraw => {}
            Msg::Repos {
                tab,
                server,
                connect,
                res,
            } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_repos(tab, server, connect, res);
            }
            Msg::Entries { tab, gen, res } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_entries(tab, gen, res);
            }
            Msg::Expand {
                tab,
                gen,
                dir_rel,
                res,
            } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_expand(tab, gen, dir_rel, res);
            }
            Msg::Walk { tab, gen, res } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_walk(tab, gen, res);
            }
            Msg::Head {
                tab,
                gen,
                name,
                res,
            } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_head(tab, gen, name, res);
            }
            Msg::Dl(ev) => self.on_dl(ev),
        }
    }

    // ---- input -----------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        // Any key but the arming one disarms the quit guard.
        let is_q = key.code == KeyCode::Char('q') && key.modifiers.is_empty();
        if !is_q {
            self.quit_armed = false;
        }
        match self.mode.clone() {
            Mode::Normal => self.on_key_normal(key),
            Mode::Help => self.on_key_help(key),
            Mode::Servers => self.on_key_servers(key),
            Mode::AddServer(buffer) => self.on_key_add(key, buffer),
        }
    }

    fn on_key_normal(&mut self, key: KeyEvent) {
        let filtering = self.tabs.get(self.tab).is_some_and(|t| t.filter.is_some());
        if filtering {
            self.on_key_filter(key);
            return;
        }
        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => self.request_quit(),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Left => self.on_left(),
            KeyCode::Right => self.on_right(),
            KeyCode::Char('k') if key.modifiers.is_empty() => self.move_cursor(-1),
            KeyCode::Char('j') if key.modifiers.is_empty() => self.move_cursor(1),
            KeyCode::PageUp => self.move_page(-1),
            KeyCode::PageDown => self.move_page(1),
            KeyCode::Home | KeyCode::Char('g') if key.modifiers.is_empty() => self.cursor_edge(0),
            KeyCode::End | KeyCode::Char('G') if key.modifiers.is_empty() => self.cursor_edge(1),
            KeyCode::Enter => self.on_enter(),
            KeyCode::Esc | KeyCode::Backspace => self.on_back(),
            KeyCode::Char('d') if key.modifiers.is_empty() => self.request_download_entry(),
            KeyCode::Char('D') if key.modifiers.is_empty() => self.request_download_dir(),
            KeyCode::Char('r') if key.modifiers.is_empty() => self.refresh(),
            KeyCode::Char('e') if key.modifiers.is_empty() => self.toggle_nav(),
            KeyCode::Char('i') if key.modifiers.is_empty() => self.request_info(),
            KeyCode::Char('s') if key.modifiers.is_empty() => self.open_servers_overlay(),
            KeyCode::Char('?') if key.modifiers.is_empty() => self.mode = Mode::Help,
            KeyCode::Char('/') if key.modifiers.is_empty() => self.start_filter(),
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::Char(c) if key.modifiers.is_empty() && c.is_ascii_digit() && c != '0' => {
                self.jump_tab(c.to_digit(10).unwrap_or(0) as usize - 1);
            }
            _ => {}
        }
    }

    /// The filter eats every key while it is active: characters extend it,
    /// backspace shortens it, esc clears it, enter keeps it.
    fn on_key_filter(&mut self, key: KeyEvent) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        let Some(filter) = t.filter.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                t.filter = None;
                t.tree_cursor = 0;
                self.info = None;
                self.status = hint_tree();
            }
            KeyCode::Enter => {
                let shown = t.row_idx().len();
                let total = t.rows().len();
                self.status = format!("filter kept: {shown} of {total}");
            }
            KeyCode::Backspace => {
                filter.pop();
                t.tree_cursor = 0;
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                filter.push(c);
                t.tree_cursor = 0;
            }
            _ => return,
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        self.status = match &t.filter {
            Some(f) => format!(
                "filter {f:?}: {} of {} shown",
                t.row_idx().len(),
                t.rows().len()
            ),
            None => hint_tree(),
        };
    }

    fn on_key_help(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
                self.mode = Mode::Normal;
            }
            _ => {}
        }
    }

    fn on_key_servers(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Char('q') if key.modifiers.is_empty() => self.request_quit(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.servers_cursor = self.servers_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.servers_cursor =
                    (self.servers_cursor + 1).min(self.servers.len().saturating_sub(1));
            }
            KeyCode::Enter => self.open_server(self.servers_cursor),
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                self.mode = Mode::AddServer(String::new());
            }
            _ => {}
        }
    }

    fn on_key_add(&mut self, key: KeyEvent, mut buffer: String) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                self.mode = Mode::AddServer(buffer.clone());
                self.submit_server(buffer);
            }
            KeyCode::Backspace => {
                buffer.pop();
                self.mode = Mode::AddServer(buffer);
            }
            KeyCode::Char(c) if key.modifiers.is_empty() => {
                buffer.push(c);
                self.mode = Mode::AddServer(buffer);
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::AddServer(buffer) => {
                buffer.extend(text.chars().filter(|c| !c.is_control()));
            }
            _ => {
                if let Some(t) = self.tabs.get_mut(self.tab) {
                    if let Some(filter) = t.filter.as_mut() {
                        filter.extend(text.chars().filter(|c| !c.is_control()));
                        t.tree_cursor = 0;
                    }
                }
            }
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_cursor(-1),
            MouseEventKind::ScrollDown => self.move_cursor(1),
            MouseEventKind::Down(MouseButton::Left) => self.on_click(mouse.row),
            _ => {}
        }
    }

    /// A left click selects the row under the pointer; a second click on the
    /// same row within [`DOUBLE_CLICK`] activates it.
    fn on_click(&mut self, row: u16) {
        if self.mode != Mode::Normal || self.list_area.width == 0 {
            return;
        }
        let top = self.list_area.y;
        if row < top || row >= top.saturating_add(self.list_area.height) {
            return;
        }
        let idx = self.list_state.offset() + (row - top) as usize;
        if idx >= self.visible_len() {
            return;
        }
        self.set_cursor(idx);
        let now = Instant::now();
        if let Some((when, prev)) = self.last_click.replace((now, idx)) {
            if prev == idx && now.duration_since(when) <= DOUBLE_CLICK {
                self.on_enter();
            }
        }
    }

    // ---- navigation ------------------------------------------------------

    /// The length of the visible list of the active screen.
    #[must_use]
    pub fn visible_len(&self) -> usize {
        self.tabs.get(self.tab).map_or(0, Tab::visible_len)
    }

    fn move_cursor(&mut self, delta: i32) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        let cursor = match t.screen {
            Screen::Repos => &mut t.repos_cursor,
            Screen::Tree => &mut t.tree_cursor,
        };
        *cursor = ((*cursor as i64) + delta as i64).clamp(0, len as i64 - 1) as usize;
        self.info = None;
    }

    fn move_page(&mut self, pages: i32) {
        let rows = self.list_area.height.max(1).saturating_sub(1) as i32;
        self.move_cursor(pages * rows);
    }

    fn cursor_edge(&mut self, edge: u8) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.set_cursor(if edge == 0 { 0 } else { len - 1 });
    }

    fn set_cursor(&mut self, idx: usize) {
        let len = self.visible_len();
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        let idx = idx.min(len.saturating_sub(1));
        match t.screen {
            Screen::Repos => t.repos_cursor = idx,
            Screen::Tree => t.tree_cursor = idx,
        }
        self.info = None;
    }

    /// Clamps both cursors of the active tab into their visible lists.
    fn sync_cursor(&mut self) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        let repos = t.repos.len();
        let entries = t.row_idx().len();
        t.repos_cursor = t.repos_cursor.min(repos.saturating_sub(1));
        t.tree_cursor = t.tree_cursor.min(entries.saturating_sub(1));
    }

    fn switch_tab(&mut self, delta: i32) {
        if self.tabs.len() < 2 {
            self.status = "one server open: add one with s".into();
            return;
        }
        let len = self.tabs.len() as i32;
        self.tab = ((self.tab as i32) + delta).rem_euclid(len) as usize;
        self.info = None;
        self.sync_cursor();
        self.status = hint_of(self.tabs[self.tab].screen);
    }

    fn jump_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        if idx == self.tab {
            return;
        }
        self.tab = idx;
        self.info = None;
        self.sync_cursor();
        self.status = hint_of(self.tabs[idx].screen);
    }

    // ---- actions ---------------------------------------------------------

    /// True while THIS tab's listing is in flight: its drills and refreshes wait.
    /// Another tab's listing never blocks here: the tabs are independent.
    fn listing_busy(&self) -> bool {
        self.tabs.get(self.tab).is_some_and(|t| t.loading)
    }

    /// True while a transfer is running: downloads are single-flight, the next
    /// one waits for the panel to resolve. Browsing never waits on it.
    fn download_busy(&self) -> bool {
        self.download.as_ref().is_some_and(Download::is_running)
    }

    fn request_quit(&mut self) {
        if self.download.as_ref().is_some_and(Download::is_running) && !self.quit_armed {
            self.quit_armed = true;
            self.status = "download in flight: press q again to quit".into();
            return;
        }
        self.quit = true;
    }

    fn on_enter(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        match t.screen {
            Screen::Repos => self.open_repo(),
            Screen::Tree => {
                let Some(row) = t.selected_row() else {
                    return;
                };
                match row.kind {
                    EntryKind::Dir => self.descend(row.name),
                    EntryKind::File => self.request_download_entry(),
                }
            }
        }
    }

    fn open_repo(&mut self) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        let Some(row) = t.repos.get(t.repos_cursor).cloned() else {
            return;
        };
        if !t.enterable(&row, self.all_formats) {
            self.status = format!(
                "format {}: not enterable, --all-formats lifts the filter",
                row.format
            );
            return;
        }
        t.repo = Some(row.clone());
        t.path.clear();
        t.entries.clear();
        t.clear_expansion();
        t.filter = None;
        t.tree_cursor = 0;
        t.gen += 1;
        t.loading = true;
        self.pending += 1;
        self.error = None;
        self.info = None;
        self.status = format!("listing {}/", row.name);
        let url = net::dir_url(&row.url);
        let (tab, gen) = (self.tab, t.gen);
        net::load_entries(self.tx.clone(), tab, gen, url, self.auth.clone());
    }

    fn descend(&mut self, name: String) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        if t.repo.is_none() {
            return;
        }
        self.status = format!("listing {name}/");
        t.path.push(name);
        let url = t.here_url();
        t.entries.clear();
        t.clear_expansion();
        t.filter = None;
        t.tree_cursor = 0;
        t.gen += 1;
        t.loading = true;
        self.pending += 1;
        self.error = None;
        self.info = None;
        let (tab, gen) = (self.tab, t.gen);
        net::load_entries(self.tx.clone(), tab, gen, url, self.auth.clone());
    }

    fn on_back(&mut self) {
        self.error = None;
        self.info = None;
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        // Listings of an abandoned drill-down are dropped when they arrive.
        t.gen += 1;
        match t.screen {
            Screen::Repos => self.status = "press q to quit".into(),
            Screen::Tree if t.path.is_empty() => {
                t.screen = Screen::Repos;
                t.entries.clear();
                t.clear_expansion();
                t.filter = None;
                t.tree_cursor = 0;
                self.status = hint_repos();
            }
            Screen::Tree => {
                t.path.pop();
                let url = t.here_url();
                t.entries.clear();
                t.filter = None;
                t.tree_cursor = 0;
                t.loading = true;
                self.pending += 1;
                self.status = format!("listing {}/", t.breadcrumb());
                let (tab, gen) = (self.tab, t.gen);
                net::load_entries(self.tx.clone(), tab, gen, url, self.auth.clone());
            }
        }
    }

    fn start_filter(&mut self) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        if t.screen != Screen::Tree {
            self.status = "the filter works in the tree".into();
            return;
        }
        t.filter = Some(String::new());
        t.tree_cursor = 0;
        self.status = "filter: type to narrow, enter keeps, esc clears".into();
    }

    /// The left arrow: mode-dependent navigation.
    fn on_left(&mut self) {
        if self
            .tabs
            .get(self.tab)
            .is_some_and(|t| t.screen == Screen::Repos)
        {
            return;
        }
        match self.nav {
            Nav::Enter => self.on_back(),
            Nav::Expand => self.collapse_or_up(),
        }
    }

    /// The right arrow: mode-dependent navigation.
    fn on_right(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        match t.screen {
            // On the repository screen right opens, in both modes.
            Screen::Repos => self.on_enter(),
            Screen::Tree => match self.nav {
                Nav::Enter => {
                    if self.listing_busy() {
                        self.status = "a listing is in flight".into();
                        return;
                    }
                    match t.selected_row() {
                        Some(row) if row.kind == EntryKind::Dir && row.depth == 0 => {
                            self.descend(row.name);
                        }
                        Some(_) => self.status = "right enters folders: this is a file".into(),
                        None => {}
                    }
                }
                Nav::Expand => self.expand_selected(),
            },
        }
    }

    /// Expand mode: the right arrow expands the selected folder inline.
    /// Collapsed-but-cached folders reopen without the network.
    fn expand_selected(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let Some(row) = t.selected_row() else {
            return;
        };
        if row.kind != EntryKind::Dir {
            self.status = "folders expand, this is a file".into();
            return;
        }
        if row.expanded {
            if let Some(t) = self.tabs.get_mut(self.tab) {
                t.open.remove(&row.rel);
            }
            self.sync_cursor();
            self.status = format!("collapsed {}/", row.rel);
            return;
        }
        if !t.expanded.contains_key(&row.rel) {
            // Not cached: load the children in the background.
            let url = format!("{}{}/", t.here_url(), row.rel);
            let (tab, gen) = (self.tab, t.gen);
            if let Some(t) = self.tabs.get_mut(self.tab) {
                t.loading_dirs.insert(row.rel.clone());
            }
            self.pending += 1;
            self.status = format!("listing {}/", row.rel);
            net::load_children(self.tx.clone(), tab, gen, row.rel, url, self.auth.clone());
            return;
        }
        if let Some(t) = self.tabs.get_mut(self.tab) {
            t.open.insert(row.rel.clone());
        }
        self.sync_cursor();
    }

    /// Expand mode, the left arrow: collapse the selected folder, or jump to
    /// its parent row, or leave the tree position entirely.
    fn collapse_or_up(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let Some(row) = t.selected_row() else {
            return;
        };
        if row.kind == EntryKind::Dir && row.expanded {
            if let Some(t) = self.tabs.get_mut(self.tab) {
                t.open.remove(&row.rel);
            }
            self.sync_cursor();
            return;
        }
        if row.depth == 0 {
            self.on_back();
            return;
        }
        // Jump to the nearest shallower visible row above the cursor.
        let cursor = t.tree_cursor;
        let rows = t.rows();
        let idx = t.row_idx();
        let Some(&current) = idx.get(cursor) else {
            return;
        };
        let depth = rows[current].depth;
        let mut target = None;
        for (position, &i) in idx.iter().take(cursor).enumerate() {
            if rows[i].depth < depth {
                target = Some(position);
            }
        }
        match target {
            Some(position) => self.set_cursor(position),
            None => self.on_back(),
        }
    }

    /// The `e` toggle: flip the left/right navigation mode and remember it.
    fn toggle_nav(&mut self) {
        self.nav = self.nav.toggled();
        for tab in &mut self.tabs {
            tab.clear_expansion();
        }
        self.sync_cursor();
        let saved = self.persist_nav();
        self.status = format!(
            "navigation: {}{}",
            self.nav.label(),
            if saved {
                ", saved to the config"
            } else {
                ", the config is not saved"
            }
        );
    }

    /// Persists the navigation mode: true when the config file took it.
    fn persist_nav(&mut self) -> bool {
        self.config.tui.nav = self.nav;
        let Some(path) = self.config_path.clone() else {
            return false;
        };
        config::save(&path, &self.config).is_ok()
    }

    fn refresh(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        t.gen += 1;
        // A refresh redraws the current listing: the inline expansion folds too.
        t.clear_expansion();
        match t.screen {
            Screen::Repos => {
                t.loading = true;
                self.pending += 1;
                self.status = format!("refreshing {}", t.server.name);
                let server = t.server.clone();
                let (slot, auth) = (self.tab, self.auth.clone());
                net::load_repos(self.tx.clone(), server, slot, false, auth);
            }
            Screen::Tree => {
                let url = t.here_url();
                t.loading = true;
                self.pending += 1;
                self.status = format!("refreshing {}/", t.breadcrumb());
                let (tab, gen, auth) = (self.tab, t.gen, self.auth.clone());
                net::load_entries(self.tx.clone(), tab, gen, url, auth);
            }
        }
    }

    /// The `i` info action: a folder reports its listing, a file HEADs its size.
    fn request_info(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if t.screen != Screen::Tree {
            return;
        }
        let Some(row) = t.selected_row() else {
            return;
        };
        let (tab, gen) = (self.tab, t.gen);
        match row.kind {
            EntryKind::Dir => {
                self.info = Some(format!(
                    "{}/ · folder, {} entries in this listing",
                    row.name,
                    t.entries.len()
                ));
            }
            EntryKind::File => {
                let url = format!("{}{}", t.here_url(), row.rel);
                let name = row.name.clone();
                self.info = Some(format!("{name}: …"));
                self.pending += 1;
                net::head_size(self.tx.clone(), tab, gen, name, url, self.auth.clone());
            }
        }
    }

    fn open_servers_overlay(&mut self) {
        // The cursor starts on the server of the active tab, when it is listed.
        if let Some(t) = self.tabs.get(self.tab) {
            self.servers_cursor = self
                .servers
                .iter()
                .position(|s| s.url == t.server.url)
                .unwrap_or(0);
        }
        self.mode = Mode::Servers;
    }

    /// Opens the server under the overlay cursor: an existing tab is focused,
    /// a new one is connected in the background.
    fn open_server(&mut self, idx: usize) {
        let Some(server) = self.servers.get(idx).cloned() else {
            return;
        };
        if let Some(i) = self.tabs.iter().position(|t| t.server.url == server.url) {
            self.tab = i;
            self.mode = Mode::Normal;
            self.sync_cursor();
            self.status = format!("tab {}: {}", i + 1, server.name);
            return;
        }
        self.connect_server(server);
    }

    /// Connects a server from the add form: normalize, then load in the background.
    fn submit_server(&mut self, raw: String) {
        let url = raw.trim().to_owned();
        if url.is_empty() {
            self.status = "the URL is empty".into();
            return;
        }
        let server = ServerCfg::from_base(net::dir_url(&url));
        if let Some(i) = self.tabs.iter().position(|t| t.server.url == server.url) {
            self.tab = i;
            self.mode = Mode::Normal;
            self.status = format!("already open as tab {}", i + 1);
            return;
        }
        self.connect_server(server);
    }

    fn connect_server(&mut self, server: ServerCfg) {
        // One connect at a time: two results for the same slot would collide.
        if self.connecting > 0 {
            self.status = "already connecting to a server".into();
            return;
        }
        self.connecting += 1;
        self.pending += 1;
        self.error = None;
        self.status = format!("connecting to {}…", server.url);
        net::load_repos(
            self.tx.clone(),
            server,
            self.tabs.len(),
            true,
            self.auth.clone(),
        );
    }

    /// The tab's server joins the config as a preset, under a fresh unique name.
    fn persist_server(&mut self, server: &ServerCfg) {
        if self.config.servers.iter().any(|s| s.url == server.url) {
            return;
        }
        let mut named = server.clone();
        let mut n = 2;
        while self.config.servers.iter().any(|s| s.name == named.name) {
            named.name = format!("{}-{n}", server.name);
            n += 1;
        }
        self.config.servers.push(named.clone());
        if !self.servers.iter().any(|s| s.url == named.url) {
            self.servers.push(named.clone());
        }
        let Some(path) = self.config_path.clone() else {
            self.status = format!(
                "tab opened, but there is no config path to save {}",
                named.name
            );
            return;
        };
        match config::save(&path, &self.config) {
            Ok(()) => {
                self.status = format!("server {} saved to {}", named.name, path.display());
            }
            Err(e) => {
                self.status = format!("tab opened, but the config was not saved: {e}");
            }
        }
    }

    // ---- downloads -------------------------------------------------------

    /// Downloads the selected entry: a folder walks its whole subtree, a file itself.
    fn request_download_entry(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        if self.download_busy() {
            self.status = "download in flight: one at a time".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let Some(repo) = t.repo.clone() else {
            return;
        };
        let Some(row) = t.selected_row() else {
            return;
        };
        let rel = row.rel;
        self.start_walk(repo, rel, row.kind);
    }

    /// Downloads the current directory: the whole subtree below the breadcrumb.
    fn request_download_dir(&mut self) {
        if self.listing_busy() {
            self.status = "a listing is in flight".into();
            return;
        }
        if self.download_busy() {
            self.status = "download in flight: one at a time".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let Some(repo) = t.repo.clone() else {
            return;
        };
        let rel = t.path.join("/");
        self.start_walk(repo, rel, EntryKind::Dir);
    }

    fn start_walk(&mut self, repo: RepoInfo, rel: String, kind: EntryKind) {
        let dst = self.download_dir.join(&repo.name);
        self.download = Some(Download {
            repo_url: net::dir_url(&repo.url),
            dst: dst.clone(),
            ..Download::default()
        });
        self.error = None;
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        t.gen += 1;
        self.pending += 1;
        self.status = match kind {
            EntryKind::Dir if rel.is_empty() => {
                format!(
                    "walking the whole repo: {} files may follow -> {}",
                    t.entries.len(),
                    dst.display()
                )
            }
            EntryKind::Dir => format!("walking {rel}/"),
            EntryKind::File => format!("plan: 1 file -> {}", dst.display()),
        };
        let (tab, gen) = (self.tab, t.gen);
        net::walk_for_download(
            self.tx.clone(),
            tab,
            gen,
            net::dir_url(&repo.url),
            rel,
            kind,
            self.auth.clone(),
        );
    }

    // ---- message application --------------------------------------------

    fn on_repos(
        &mut self,
        slot: usize,
        server: ServerCfg,
        connect: bool,
        res: Result<Vec<RepoInfo>, Error>,
    ) {
        if connect {
            self.connecting = self.connecting.saturating_sub(1);
        }
        match res {
            Ok(repos) if slot < self.tabs.len() => {
                // A refresh of an existing tab.
                let t = &mut self.tabs[slot];
                let count = repos.len();
                t.repos = repos;
                t.repos_cursor = 0;
                t.loading = false;
                // An opened repository may have vanished from the server.
                if let Some(open) = &t.repo {
                    if !t.repos.iter().any(|r| r.url == open.url) {
                        t.repo = None;
                        t.screen = Screen::Repos;
                        t.path.clear();
                        t.entries.clear();
                    }
                }
                if self.tab == slot {
                    self.status = format!("{count} repositories on {}", t.server.name);
                }
            }
            Ok(_repos) if self.tabs.iter().any(|t| t.server.url == server.url) => {
                // A duplicate connect: focus the tab that is already open.
                if let Some(i) = self.tabs.iter().position(|t| t.server.url == server.url) {
                    self.tab = i;
                }
                self.mode = Mode::Normal;
                self.sync_cursor();
                self.status = format!("already open as tab {}", self.tab + 1);
            }
            Ok(repos) => {
                // A new server: the tab opens and the server joins the config.
                let count = repos.len();
                self.tabs.push(Tab::new(server.clone(), repos));
                self.tab = self.tabs.len() - 1;
                self.mode = Mode::Normal;
                self.status = format!(
                    "{}: {count} repositories, tab {} of {}",
                    server.name,
                    self.tabs.len(),
                    self.tabs.len()
                );
                self.persist_server(&server);
            }
            Err(e) => {
                // A refresh of an existing tab must not leave the loading marker on.
                if slot < self.tabs.len() {
                    self.tabs[slot].loading = false;
                }
                // The overlay stays open with the input preserved.
                self.error = Some((format!("server {}: {e}", server.name), e.hint()));
            }
        }
    }

    fn on_entries(&mut self, tab: usize, gen: u64, res: Result<Vec<Entry>, Error>) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.loading = false;
        if gen != t.gen {
            return;
        }
        match res {
            Ok(entries) => {
                let empty = entries.is_empty();
                t.entries = entries;
                t.tree_cursor = 0;
                t.filter = None;
                t.screen = Screen::Tree;
                self.info = None;
                if self.tab == tab {
                    self.status = if empty {
                        "the folder is empty, esc goes back up".into()
                    } else {
                        hint_tree()
                    };
                }
            }
            Err(e) => {
                if self.tab == tab {
                    self.fail_err(e);
                }
            }
        }
    }

    /// Folds an expansion result into the tree: the folder opens with its
    /// cached children; a stale result only drops the loading marker.
    fn on_expand(&mut self, tab: usize, gen: u64, dir_rel: String, res: Result<Vec<Entry>, Error>) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.loading_dirs.remove(&dir_rel);
        if gen != t.gen {
            return;
        }
        match res {
            Ok(children) => {
                let count = children.len();
                t.expanded.insert(dir_rel.clone(), children);
                t.open.insert(dir_rel);
                if self.tab == tab {
                    self.sync_cursor();
                    self.status = format!("expanded: {count} entries");
                }
            }
            Err(e) => {
                if self.tab == tab {
                    self.fail_err(e);
                }
            }
        }
    }

    fn on_walk(&mut self, tab: usize, gen: u64, res: Result<Vec<ArtifactName>, Error>) {
        let fresh = self.tabs.get(tab).is_some_and(|t| gen == t.gen);
        if !fresh {
            // The tab navigated away while the walk ran: the request is void,
            // and the download it prepared must not stay "running" forever.
            self.download = None;
            return;
        }
        match res {
            Ok(names) if names.is_empty() => {
                if let Some(dl) = self.download.as_mut() {
                    dl.outcome = Some(DlOutcome::Failed("the subtree holds no files".into(), None));
                }
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
                net::start_download(self.tx.clone(), repo_url, names, dst, self.auth.clone());
            }
            Err(e) => {
                if let Some(dl) = self.download.as_mut() {
                    dl.outcome = Some(DlOutcome::Failed(e.to_string(), e.hint()));
                }
                self.fail_err(e);
            }
        }
    }

    fn on_head(&mut self, tab: usize, gen: u64, name: String, res: Result<HeadInfo, Error>) {
        if tab != self.tab {
            return;
        }
        let fresh = self.tabs.get(tab).is_some_and(|t| gen == t.gen);
        if !fresh {
            return;
        }
        self.info = Some(match res {
            Ok(info) => match info.size {
                Some(size) => {
                    let ct = info.content_type.as_deref().unwrap_or("binary");
                    format!("{name}: {} · {ct}", fmt_bytes(size))
                }
                None => format!("{name}: size unknown (no content-length)"),
            },
            Err(e) => format!("{name}: head failed: {e}"),
        });
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

    fn fail_err(&mut self, e: Error) {
        self.error = Some((e.to_string(), e.hint()));
    }
}

/// The idle status hint of a screen.
#[must_use]
pub fn hint_of(screen: Screen) -> String {
    match screen {
        Screen::Repos => hint_repos(),
        Screen::Tree => hint_tree(),
    }
}

/// The idle status hint of the repository screen.
#[must_use]
pub fn hint_repos() -> String {
    "enter: open · r: refresh · s: servers · tab: switch · ?: help · q: quit".into()
}

/// The idle status hint of the tree screen.
#[must_use]
pub fn hint_tree() -> String {
    "d: download · D: folder · /: filter · e: mode · i: info · r: refresh · esc: up · q: quit"
        .into()
}

/// Formats a byte count for status and info lines: `834 B`, `1.2 MiB`.
#[must_use]
pub fn fmt_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let bytes = n as f64;
    if bytes < KIB {
        return format!("{n} B");
    }
    let mut value = bytes;
    for unit in ["KiB", "MiB", "GiB", "TiB"] {
        value /= KIB;
        if value < KIB {
            return format!("{value:.1} {unit}");
        }
    }
    format!("{value:.1} PiB")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> ServerCfg {
        ServerCfg {
            name: "127.0.0.1:1".into(),
            url: "http://127.0.0.1:1/".into(),
        }
    }

    fn repo(name: &str, format: &str, kind: &str) -> RepoInfo {
        RepoInfo {
            name: name.to_owned(),
            format: format.to_owned(),
            kind: kind.to_owned(),
            url: format!("http://127.0.0.1:1/repository/{name}/"),
        }
    }

    fn tab_with(repos: Vec<RepoInfo>) -> Tab {
        Tab::new(server(), repos)
    }

    fn app_with(tabs: Vec<Tab>, all_formats: bool) -> (App, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut app = App::new(
            vec![server()],
            tabs,
            ConfigFile::default(),
            None,
            all_formats,
            PathBuf::from("dl"),
            None,
            tx,
        );
        app.boot();
        (app, rx)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn tree_tab(entries: Vec<Entry>) -> Tab {
        let mut tab = tab_with(vec![repo("raw-main", "raw", "hosted")]);
        tab.repo = tab.repos.first().cloned();
        tab.screen = Screen::Tree;
        tab.entries = entries;
        tab
    }

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: name.to_owned(),
            kind,
        }
    }

    #[test]
    fn only_raw_is_enterable_by_default() {
        let tab = tab_with(vec![repo("raw-main", "raw", "hosted")]);
        assert!(tab.enterable(&repo("raw-main", "raw", "hosted"), false));
        assert!(!tab.enterable(&repo("maven-central", "maven2", "proxy"), false));
        assert!(tab.enterable(&repo("maven-central", "maven2", "proxy"), true));
    }

    #[tokio::test]
    async fn arrows_move_the_selection() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![
                repo("a", "raw", "hosted"),
                repo("b", "raw", "hosted"),
                repo("c", "raw", "hosted"),
            ])],
            false,
        );
        // Down and j move, clamped at the last row.
        app.handle(Msg::Key(key(KeyCode::Down)));
        assert_eq!(app.tabs[0].repos_cursor, 1);
        app.handle(Msg::Key(key(KeyCode::Char('j'))));
        assert_eq!(app.tabs[0].repos_cursor, 2);
        app.handle(Msg::Key(key(KeyCode::Down)));
        assert_eq!(app.tabs[0].repos_cursor, 2, "clamped at the last row");
        // Up and k move back, clamped at the first row.
        app.handle(Msg::Key(key(KeyCode::Up)));
        assert_eq!(app.tabs[0].repos_cursor, 1);
        app.handle(Msg::Key(key(KeyCode::Char('k'))));
        assert_eq!(app.tabs[0].repos_cursor, 0);
        app.handle(Msg::Key(key(KeyCode::Up)));
        assert_eq!(app.tabs[0].repos_cursor, 0, "clamped at the first row");
        // End and Home, vim style g and G.
        app.handle(Msg::Key(key(KeyCode::End)));
        assert_eq!(app.tabs[0].repos_cursor, 2);
        app.handle(Msg::Key(key(KeyCode::Home)));
        assert_eq!(app.tabs[0].repos_cursor, 0);
        app.handle(Msg::Key(key(KeyCode::Char('G'))));
        assert_eq!(app.tabs[0].repos_cursor, 2);
        app.handle(Msg::Key(key(KeyCode::Char('g'))));
        assert_eq!(app.tabs[0].repos_cursor, 0);
    }

    #[tokio::test]
    async fn arrows_move_the_tree_selection_too() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("docs", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Down)));
        assert_eq!(app.tabs[0].tree_cursor, 1);
        app.handle(Msg::Key(key(KeyCode::Up)));
        assert_eq!(app.tabs[0].tree_cursor, 0);
    }

    #[tokio::test]
    async fn a_filter_narrows_the_tree_and_esc_clears_it() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("core", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert_eq!(app.status, "filter \"c\": 1 of 3 shown");
        // The cursor selects within the filtered list.
        assert_eq!(app.tabs[0].tree_cursor, 0);
        assert_eq!(
            app.tabs[0].selected_row().map(|r| r.name),
            Some("core".to_owned())
        );
        // Enter keeps the filter, esc clears it.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.tabs[0].filter.is_some());
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert!(app.tabs[0].filter.is_none());
        assert_eq!(app.visible_len(), 3);
    }

    #[tokio::test]
    async fn enter_on_a_non_raw_repo_only_reports() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("maven-central", "maven2", "proxy")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.tabs[0].screen, Screen::Repos);
        assert!(app.status.contains("maven2"));
        assert!(app.status.contains("--all-formats"));
        assert!(rx.try_recv().is_err(), "no load was spawned");
    }

    #[tokio::test]
    async fn enter_on_a_non_raw_repo_opens_under_all_formats() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("maven-central", "maven2", "proxy")])],
            true,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(
            app.tabs[0].repo.as_ref().map(|r| r.name.as_str()),
            Some("maven-central")
        );
        match rx.recv().await.unwrap() {
            Msg::Entries { res, .. } => assert!(res.is_err(), "the mock port refuses"),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn enter_on_a_raw_repo_lists_the_tree() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.pending, 1);
        match rx.recv().await.unwrap() {
            Msg::Entries { tab, res, .. } => {
                assert_eq!(tab, 0);
                assert!(res.is_err(), "the mock port refuses");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tree_navigation_descends_and_ascends() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        assert_eq!(app.tabs[0].breadcrumb(), "raw-main");

        // Enter on the first entry: the folder opens and the breadcrumb grows.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.tabs[0].path, vec!["app".to_owned()]);
        assert_eq!(app.tabs[0].breadcrumb(), "raw-main / app");
        assert_eq!(app.pending, 1);
        assert_eq!(
            app.tabs[0].here_url(),
            "http://127.0.0.1:1/repository/raw-main/app/"
        );

        // The child listing lands: `core/` with a file inside.
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![entry("core", EntryKind::Dir)]),
        });
        assert_eq!(app.tabs[0].tree_cursor, 0);

        // Backspace climbs one level and reloads the parent listing.
        app.handle(Msg::Key(key(KeyCode::Backspace)));
        assert!(app.tabs[0].path.is_empty());
        assert_eq!(app.pending, 1);
        assert_eq!(
            app.tabs[0].here_url(),
            "http://127.0.0.1:1/repository/raw-main/"
        );

        // Esc on the repo root returns to the repositories.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.tabs[0].screen, Screen::Repos);
    }

    #[tokio::test]
    async fn tab_switching_preserves_each_tabs_state() {
        let (mut app, _rx) = app_with(
            vec![
                tree_tab(vec![entry("app", EntryKind::Dir)]),
                tab_with(vec![repo("other", "raw", "hosted")]),
            ],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Tab)));
        assert_eq!(app.tab, 1);
        assert_eq!(app.tabs[1].screen, Screen::Repos);
        app.handle(Msg::Key(key(KeyCode::BackTab)));
        assert_eq!(app.tab, 0);
        assert_eq!(app.tabs[0].screen, Screen::Tree);
        assert_eq!(app.tabs[0].entries.len(), 1);
        // Digit jumping.
        app.handle(Msg::Key(key(KeyCode::Char('2'))));
        assert_eq!(app.tab, 1);
        app.handle(Msg::Key(key(KeyCode::Char('1'))));
        assert_eq!(app.tab, 0);
        // Out of range digits are ignored.
        app.handle(Msg::Key(key(KeyCode::Char('9'))));
        assert_eq!(app.tab, 0);
    }

    #[tokio::test]
    async fn stale_listings_are_dropped() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        // The user backs out before the listing lands: the generation moved on.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        let Msg::Entries { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the listing");
        };
        assert_ne!(gen, app.tabs[tab].gen, "the stale result must be dropped");
        let pending_after_back = app.pending;
        app.handle(Msg::Entries {
            tab,
            gen,
            res: Ok(vec![entry("app", EntryKind::Dir)]),
        });
        assert_eq!(
            app.tabs[0].screen,
            Screen::Repos,
            "the stale result never opens a screen"
        );
        // And it did not leak the in-flight counter.
        assert_eq!(app.pending, pending_after_back - 1);
    }

    #[tokio::test]
    async fn enter_on_a_file_downloads_it_directly() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.status, "plan: 1 file -> dl/raw-main");
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
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
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
    async fn download_dir_grabs_the_whole_current_directory() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        // Descend into app/ and let the child listing land.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![entry("core", EntryKind::Dir)]),
        });
        // D downloads the folder itself: the walk runs over app/ below the root.
        app.handle(Msg::Key(key(KeyCode::Char('D'))));
        assert_eq!(app.status, "walking app/");
        assert_eq!(app.pending, 1);
        // The real listing of the descend (the dead port refuses) races the
        // walk result through the channel: drain until the walk arrives.
        let mut msg = rx.recv().await.unwrap();
        while matches!(msg, Msg::Entries { .. }) {
            msg = rx.recv().await.unwrap();
        }
        match msg {
            Msg::Walk { res, .. } => {
                // The mock port refuses: the walk itself did start for app/.
                assert!(res.is_err());
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn download_plan_starts_the_transfer() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let Msg::Walk { tab, gen, res } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        app.handle(Msg::Walk { tab, gen, res });
        assert_eq!(app.status, "plan: 1 file -> dl/raw-main");
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { files, .. }) => assert_eq!(files, 1),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_stale_walk_never_starts_a_transfer() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        // A walk of an older generation lands: dropped, nothing transfers,
        // and the download it prepared does not stay "running" forever.
        app.handle(Msg::Walk {
            tab: 0,
            gen: app.tabs[0].gen - 1,
            res: Ok(vec![ArtifactName::parse("README.txt").unwrap()]),
        });
        assert!(rx.try_recv().is_err(), "no download was spawned");
        assert!(app.download.is_none(), "the stale walk released the slot");
        assert!(!app.download_busy(), "the app is usable again");
    }

    #[tokio::test]
    async fn a_failed_walk_releases_the_app() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let Msg::Walk { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        // The walk refused (the mock port is dead): the panel resolves as failed.
        app.handle(Msg::Walk {
            tab,
            gen,
            res: Err(Error::Transport {
                url: "http://127.0.0.1:1/".into(),
                detail: "connection refused".into(),
            }),
        });
        let Some(dl) = app.download.as_ref() else {
            panic!("the download panel exists");
        };
        assert!(matches!(dl.outcome, Some(DlOutcome::Failed(..))));
        assert!(
            !app.download_busy(),
            "a new download can start after the failure"
        );
    }

    #[tokio::test]
    async fn an_empty_walk_releases_the_app() {
        let (mut app, mut rx) =
            app_with(vec![tree_tab(vec![entry("empty", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let Msg::Walk { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        app.handle(Msg::Walk {
            tab,
            gen,
            res: Ok(vec![]),
        });
        let Some(dl) = app.download.as_ref() else {
            panic!("the download panel exists");
        };
        assert!(matches!(dl.outcome, Some(DlOutcome::Failed(..))));
        assert!(
            !app.download_busy(),
            "a new download can start after the empty walk"
        );
    }

    #[tokio::test]
    async fn a_refused_repos_refresh_clears_the_loading_marker() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('r'))));
        assert!(app.tabs[0].loading);
        app.handle(Msg::Repos {
            tab: 0,
            server: server(),
            connect: false,
            res: Err(Error::Transport {
                url: "http://127.0.0.1:1/".into(),
                detail: "connection refused".into(),
            }),
        });
        assert!(!app.tabs[0].loading, "no phantom loading marker");
        assert!(app.error.is_some());
    }

    #[tokio::test]
    async fn browsing_continues_while_a_download_runs() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        // A download of the file starts and runs.
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let Msg::Walk { tab, gen, res } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        app.handle(Msg::Walk { tab, gen, res });
        let Msg::Dl(DlEv::Start { files, .. }) = rx.recv().await.unwrap() else {
            panic!("expected the transfer start");
        };
        assert_eq!(files, 1);
        assert!(app.download_busy());
        // Browsing does not wait on the transfer: the folder above opens.
        app.handle(Msg::Key(key(KeyCode::Up)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.tabs[0].loading, "the descent listing spawned");
        // The listing lands, then a second download politely waits its turn.
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![]),
        });
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert_eq!(app.status, "download in flight: one at a time");
        assert!(rx.try_recv().is_err(), "no second walk was spawned");
    }

    #[tokio::test]
    async fn a_second_connect_while_connecting_is_refused() {
        let (mut app, mut rx) = app_with(vec![tab_with(vec![])], false);
        app.mode = Mode::AddServer("http://x:2/".into());
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.connecting, 1);
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.connecting, 1);
        assert_eq!(app.status, "already connecting to a server");
        assert!(rx.try_recv().is_err(), "no second connect was spawned");
        app.handle(Msg::Repos {
            tab: 1,
            server: ServerCfg::from_base("http://x:2/".to_owned()),
            connect: true,
            res: Ok(vec![repo("raw", "raw", "hosted")]),
        });
        assert_eq!(app.connecting, 0);
    }

    #[tokio::test]
    async fn arrows_enter_and_return_in_enter_mode() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        assert_eq!(app.nav, Nav::Enter);
        // Right on the folder enters it, exactly like enter.
        app.handle(Msg::Key(key(KeyCode::Right)));
        assert_eq!(app.tabs[0].path, vec!["app".to_owned()]);
        assert_eq!(app.pending, 1);
        // Left climbs back up.
        app.handle(Msg::Key(key(KeyCode::Left)));
        assert!(app.tabs[0].path.is_empty());
        // The reload lands, then right on a file only reports.
        let Msg::Entries { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the listing");
        };
        app.handle(Msg::Entries {
            tab,
            gen,
            res: Ok(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ]),
        });
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Right)));
        assert!(app.status.contains("this is a file"), "{:?}", app.status);
    }

    #[tokio::test]
    async fn expand_mode_expands_collapses_and_jumps() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('e'))));
        assert_eq!(app.nav, Nav::Expand);
        // Right on the folder spawns the expansion load.
        app.handle(Msg::Key(key(KeyCode::Right)));
        assert!(app.tabs[0].loading_dirs.contains("app"));
        assert_eq!(app.pending, 1);
        // The real load dies against the dead port: drain it, then land ours.
        let Msg::Expand {
            tab, gen, dir_rel, ..
        } = rx.recv().await.unwrap()
        else {
            panic!("expected the expansion");
        };
        assert_eq!((tab, dir_rel.as_str()), (0, "app"));
        app.handle(Msg::Expand {
            tab,
            gen,
            dir_rel: "app".into(),
            res: Ok(vec![
                entry("core", EntryKind::Dir),
                entry("lib.rs", EntryKind::File),
            ]),
        });
        // The children sit inline under their folder, one level deeper.
        let rows = app.tabs[0].rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].rel, "app/core");
        assert_eq!(rows[1].depth, 1);
        assert!(!rows[1].expanded);
        // Left on the child jumps to the parent row.
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Left)));
        assert_eq!(app.tabs[0].tree_cursor, 0);
        // Right on the expanded folder collapses it; right again reopens it
        // from the cache without another load.
        app.handle(Msg::Key(key(KeyCode::Right)));
        assert!(!app.tabs[0].open.contains("app"));
        assert_eq!(app.tabs[0].rows().len(), 2);
        app.handle(Msg::Key(key(KeyCode::Right)));
        assert!(app.tabs[0].open.contains("app"));
        assert_eq!(app.tabs[0].rows().len(), 4);
        assert!(rx.try_recv().is_err(), "the cache reopened without a load");
    }

    #[tokio::test]
    async fn the_mode_toggle_persists_into_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(
            vec![server()],
            vec![tab_with(vec![])],
            ConfigFile::default(),
            Some(path.clone()),
            false,
            PathBuf::from("dl"),
            None,
            tx,
        );
        app.boot();
        app.handle(Msg::Key(key(KeyCode::Char('e'))));
        assert_eq!(app.nav, Nav::Expand);
        assert!(
            app.status.contains("saved to the config"),
            "{:?}",
            app.status
        );
        assert_eq!(crate::config::load(&path).unwrap().tui.nav, Nav::Expand);
        app.handle(Msg::Key(key(KeyCode::Char('e'))));
        assert_eq!(app.nav, Nav::Enter);
        assert_eq!(crate::config::load(&path).unwrap().tui.nav, Nav::Enter);
    }

    #[tokio::test]
    async fn refresh_reloads_the_current_screen() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('r'))));
        assert_eq!(app.pending, 1);
        match rx.recv().await.unwrap() {
            Msg::Repos { tab, res, .. } => {
                assert_eq!(tab, 0);
                assert!(res.is_err(), "the mock port refuses");
            }
            other => panic!("unexpected message: {other:?}"),
        }
        // The result of the refresh replaces the repository list.
        app.handle(Msg::Repos {
            tab: 0,
            server: server(),
            connect: false,
            res: Ok(vec![repo("fresh", "raw", "hosted")]),
        });
        assert_eq!(app.tabs[0].repos.len(), 1);
        assert_eq!(app.tabs[0].repos[0].name, "fresh");
    }

    #[tokio::test]
    async fn info_heads_the_selected_file() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('i'))));
        assert_eq!(app.pending, 1);
        assert_eq!(app.info.as_deref(), Some("README.txt: …"));
        app.handle(Msg::Head {
            tab: 0,
            gen: app.tabs[0].gen,
            name: "README.txt".into(),
            res: Ok(HeadInfo {
                status: 200,
                size: Some(21),
                content_type: Some("text/plain".into()),
            }),
        });
        assert_eq!(app.info.as_deref(), Some("README.txt: 21 B · text/plain"));
        assert_eq!(app.pending, 0);
    }

    #[tokio::test]
    async fn the_quit_guard_needs_two_presses_while_downloading() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.download = Some(Download::default());
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert!(!app.quit);
        assert!(app.quit_armed);
        assert!(app.status.contains("press q again"));
        // Any other key disarms.
        app.handle(Msg::Key(key(KeyCode::Down)));
        assert!(!app.quit_armed);
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert!(!app.quit);
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert!(app.quit);
        // ctrl-c always quits at once.
        app.quit = false;
        app.handle(Msg::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.quit);
    }

    #[tokio::test]
    async fn the_add_server_form_types_and_connects() {
        let (mut app, mut rx) = app_with(vec![tab_with(vec![])], false);
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        app.handle(Msg::Key(key(KeyCode::Char('a'))));
        for c in "http://x:2/".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        assert_eq!(app.mode, Mode::AddServer("http://x:2/".into()));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.pending, 1);
        match rx.recv().await.unwrap() {
            Msg::Repos { tab, server, .. } => {
                assert_eq!(tab, 1, "the new tab slots after the existing one");
                assert_eq!(server.url, "http://x:2/");
            }
            other => panic!("unexpected message: {other:?}"),
        }
        // The listing lands: the tab opens, the mode returns to normal.
        app.handle(Msg::Repos {
            tab: 1,
            server: ServerCfg::from_base("http://x:2/".to_owned()),
            connect: true,
            res: Ok(vec![repo("raw", "raw", "hosted")]),
        });
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tab, 1);
        assert_eq!(app.mode, Mode::Normal);
        // With no config path the preset stays in memory only, with a note.
        assert!(app.status.contains("no config path"), "{:?}", app.status);
        // The servers overlay now knows both.
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        assert_eq!(app.servers.len(), 2);
    }

    #[tokio::test]
    async fn an_added_server_persists_into_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut app = App::new(
            vec![server()],
            vec![tab_with(vec![])],
            ConfigFile::default(),
            Some(path.clone()),
            false,
            PathBuf::from("dl"),
            None,
            tx,
        );
        app.boot();
        app.handle(Msg::Repos {
            tab: 1,
            server: ServerCfg::from_base("http://x:2/".to_owned()),
            connect: true,
            res: Ok(vec![repo("raw", "raw", "hosted")]),
        });
        assert!(app.status.contains("saved to"), "{:?}", app.status);
        let saved = crate::config::load(&path).unwrap();
        assert_eq!(saved.servers.len(), 1);
        assert_eq!(saved.servers[0].url, "http://x:2/");
    }

    #[tokio::test]
    async fn the_servers_overlay_switches_between_open_tabs() {
        let (mut app, _rx) = app_with(
            vec![
                tab_with(vec![repo("raw-main", "raw", "hosted")]),
                tab_with(vec![repo("raw-two", "raw", "hosted")]),
            ],
            false,
        );
        // Both tabs share the same server URL in this fixture, so seed the
        // second tab with a distinct one.
        app.tabs[1].server = ServerCfg {
            name: "two".into(),
            url: "http://two:2/".into(),
        };
        app.servers.push(app.tabs[1].server.clone());
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        assert_eq!(app.mode, Mode::Servers);
        // Enter on the first server focuses its tab.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.tab, 0);
        // Move the cursor to the second server and enter: tab 1 opens.
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.tab, 1);
    }

    #[tokio::test]
    async fn the_wheel_moves_the_selection() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![
                repo("a", "raw", "hosted"),
                repo("b", "raw", "hosted"),
            ])],
            false,
        );
        app.handle(Msg::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::empty(),
        }));
        assert_eq!(app.tabs[0].repos_cursor, 1);
        app.handle(Msg::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::empty(),
        }));
        assert_eq!(app.tabs[0].repos_cursor, 0);
    }

    #[tokio::test]
    async fn a_click_selects_and_a_double_click_opens() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![
                repo("raw-main", "raw", "hosted"),
                repo("raw-two", "raw", "hosted"),
            ])],
            false,
        );
        // The frame geometry: the list inner area starts at row 1.
        app.list_area = Rect {
            x: 0,
            y: 1,
            width: 40,
            height: 5,
        };
        let click = |row: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row,
            modifiers: KeyModifiers::empty(),
        };
        app.handle(Msg::Mouse(click(2)));
        assert_eq!(
            app.tabs[0].repos_cursor, 1,
            "row 2 is the second visible row"
        );
        app.handle(Msg::Mouse(click(2)));
        // The tab switched screens: the second click was the double click.
        assert_eq!(app.pending, 1, "the double click opened the repository");
    }

    #[tokio::test]
    async fn paste_fills_the_add_server_form() {
        let (mut app, _rx) = app_with(vec![tab_with(vec![])], false);
        app.mode = Mode::AddServer("http://".into());
        app.handle(Msg::Paste("127.0.0.1:9/\n".into()));
        assert_eq!(app.mode, Mode::AddServer("http://127.0.0.1:9/".into()));
    }

    #[test]
    fn byte_formatting_climbs_the_units() {
        assert_eq!(fmt_bytes(834), "834 B");
        assert_eq!(fmt_bytes(1024), "1.0 KiB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(fmt_bytes(3 * 1024 * 1024 + 120 * 1024), "3.1 MiB");
    }
}
