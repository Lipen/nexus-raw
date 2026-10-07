//! TUI state: tabs, screens, overlays and the message handlers that mutate them.
//!
//! One [`Tab`] per server holds its own screen, cursor and generation counter,
//! so listings of different servers never cancel each other. The app folds
//! [`Msg`] values: key presses, mouse events, listing results and download
//! progress. Rendering lives in [`crate::ui`], the terminal loop in [`crate::tui`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// A bare action key: capitals arrive with Shift, so Shift is allowed.
/// Control, Alt and Super combos never trigger actions.
fn pressed(key: &KeyEvent, expected: char) -> bool {
    key.code == KeyCode::Char(expected)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

/// A text character for an input field: Shift is part of the character,
/// so uppercase paths and names type as typed.
fn text_char(key: &KeyEvent) -> Option<char> {
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        _ => None,
    }
}
use nexus_raw_core::service::RepoInfo;
use nexus_raw_core::{ArtifactName, Dir, Entry, EntryKind, Error, HeadInfo, Summary};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;

use crate::clipboard;
use crate::config::{self, host_of, ConfigFile, Nav, ServerCfg};
use crate::net;

/// A double click is two clicks on the same row within this window.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How long a toast stays on screen.
const TOAST_TTL: Duration = Duration::from_millis(2000);

/// How many toasts stack at once.
const TOAST_STACK: usize = 3;

/// The narrowest terminal dual-pane opens on: two panes of forty columns.
const MIN_DUAL_COLS: u16 = 80;

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
    /// The applied filter. An empty filter is off: no zombie state.
    pub filter: Option<String>,
    /// True while the filter input is open: `/` opens it, enter applies and
    /// closes, esc clears and closes, backspace to empty closes it off.
    pub filter_edit: bool,
    /// The row name the cursor sat on when a refresh started: the landing
    /// listing puts the cursor back on it instead of resetting to the top.
    pub keep_cursor_name: Option<String>,
    /// Request generation of this tab: results of an older generation are dropped.
    pub gen: u64,
    /// True while a listing of the current position is in flight.
    pub loading: bool,
    /// True when the in-flight listing is the quiet refresh of a finished
    /// put: its landing keeps the status line alone.
    pub refresh_quiet: bool,
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
            filter_edit: false,
            keep_cursor_name: None,
            gen: 0,
            loading: false,
            refresh_quiet: false,
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

/// Terminal outcome of one transfer.
#[derive(Debug)]
pub enum DlOutcome {
    /// The summary of a finished transfer.
    Done(Summary),
    /// The message and hint of a refusal.
    Failed(String, Option<String>),
    /// The user aborted the run with `x`.
    Cancelled,
}

/// A repository flattened to plain fields, so cards and dialogs can hold,
/// compare and clone it without borrowing the tab's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    /// Repository name.
    pub name: String,
    /// Repository format (`raw`, `maven2`, ...).
    pub format: String,
    /// Repository kind (`hosted`, `proxy`, `group`).
    pub kind: String,
    /// Repository URL.
    pub url: String,
}

impl From<&RepoInfo> for RepoRow {
    fn from(repo: &RepoInfo) -> Self {
        Self {
            name: repo.name.clone(),
            format: repo.format.clone(),
            kind: repo.kind.clone(),
            url: repo.url.clone(),
        }
    }
}

impl RepoRow {
    /// The directory URL every request of this repository resolves against.
    #[must_use]
    pub fn dir_url(&self) -> String {
        net::dir_url(&self.url)
    }
}

/// The size line of a file card while its HEAD is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizeState {
    /// The HEAD has not landed yet.
    Pending,
    /// The server sent no Content-Length.
    Unknown,
    /// The Content-Length arrived.
    Known(u64),
    /// The HEAD failed.
    Failed(String),
}

/// The sha256 line of a file card while its `.sha256` sibling is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaState {
    /// The sibling GET has not landed yet.
    Pending,
    /// The server has no `.sha256` marker.
    Absent,
    /// The marker parsed: the pinned digest.
    Hex(String),
    /// The marker fetch failed or the marker does not parse.
    Failed(String),
}

/// The card over one file of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCard {
    /// The tab the file lives in: part of the HEAD row token.
    pub tab: usize,
    /// The generation at open time: part of the HEAD row token.
    pub gen: u64,
    /// The path from the repository root: the row token and the target name.
    pub rel: String,
    /// The file name.
    pub name: String,
    /// The full URL of the file.
    pub url: String,
    /// The repository the file belongs to.
    pub repo: RepoRow,
    /// The HEAD outcome.
    pub size: SizeState,
    /// The `.sha256` sibling outcome.
    pub sha: ShaState,
}

/// The card over one repository row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCard {
    /// The repository details.
    pub repo: RepoRow,
    /// Whether the repository can be opened under the current format filter.
    pub enterable: bool,
}

/// One card: a repository inspection or a file inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Card {
    /// The card over a repository row of the Repos screen.
    Repo(RepoCard),
    /// The card over a file of the Tree screen.
    File(Box<FileCard>),
}

impl Card {
    /// The URL the `c` copy action carries: the file or the repository.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Card::Repo(c) => &c.repo.url,
            Card::File(c) => &c.url,
        }
    }
}

/// The destination picker: a path buffer with an optional deferred action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestPicker {
    /// The path typed so far, prefilled with the current destination.
    pub buffer: String,
    /// The card to return to on close, when `o` came from inside a card.
    pub back_to_card: Option<Card>,
    /// The repository download deferred until the destination is confirmed.
    pub pending_repo: Option<RepoRow>,
}

/// The source picker of a put: a local folder to upload from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickSource {
    /// The path typed so far, prefilled with the last upload source.
    pub buffer: String,
}

/// One row of the local pane: a filesystem child with its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    /// The file or folder name.
    pub name: String,
    /// Folder or file.
    pub kind: EntryKind,
    /// The byte size of a file.
    /// Folders carry none.
    pub size: Option<u64>,
}

/// The local pane of dual-pane browsing: one folder of the filesystem.
#[derive(Debug)]
pub struct LocalPane {
    /// The folder this pane shows.
    pub cwd: PathBuf,
    /// The children of the folder, folders first, names sorted.
    pub entries: Vec<LocalEntry>,
    /// The cursor over the children.
    pub cursor: usize,
    /// True while a listing of the folder is in flight.
    pub loading: bool,
}

/// Which field of the download-as dialog the keystrokes edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveAsField {
    /// The target folder.
    Dir,
    /// The top-level name below the folder.
    Name,
}

/// The subtree a download-as dialog downloads: the repository and the path
/// below its root, empty for the whole repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DlScope {
    /// The repository.
    pub repo: RepoRow,
    /// The path from the repository root, empty for the whole repository.
    pub rel: String,
}

/// The download-as dialog: folder and name, nothing written until Enter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAsDialog {
    /// The target folder, prefilled with the session destination.
    pub dir: String,
    /// The top-level name, prefilled with the folder or repository name.
    pub name: String,
    /// The field the keystrokes go to.
    pub field: SaveAsField,
    /// What the download covers.
    pub scope: DlScope,
}

/// What `r` in the error modal runs again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    /// Refresh the current screen of one tab.
    Listing {
        /// The tab to refresh.
        tab: usize,
    },
    /// Connect the same server again.
    Connect {
        /// The server preset.
        server: ServerCfg,
    },
    /// Walk the same scope again and transfer into the same destination.
    Walk {
        /// The repository.
        repo: RepoRow,
        /// The path from the repository root, empty for the whole repository.
        rel: String,
        /// The destination folder the transfer lands in.
        dst: PathBuf,
        /// Whether the names strip to the scope (the dialog's rename target).
        scope_relative: bool,
    },
    /// Get the same file again.
    Get {
        /// The file URL.
        url: String,
        /// The local target path of the file.
        out: PathBuf,
        /// The destination folder, for the panel.
        dst: PathBuf,
        /// The repository, for the facts.
        repo: RepoRow,
    },
    /// Re-run the same transfer plan.
    Transfer {
        /// The repository URL the names are relative to.
        repo_url: String,
        /// The names to download.
        names: Vec<ArtifactName>,
        /// The destination folder.
        dst: PathBuf,
    },
    /// Upload the same local folder again.
    Put {
        /// The local source folder.
        src: PathBuf,
        /// The remote base URL the folder uploads into.
        base: String,
    },
}

/// The error modal: cause, facts, next steps, full text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorModal {
    /// The class headline: download failed, listing failed, connect failed,
    /// access denied.
    pub title: &'static str,
    /// One human sentence without colon chains.
    pub cause: String,
    /// Non-empty `key: value` facts: server, repository, names, destination,
    /// HTTP status.
    pub facts: Vec<(String, String)>,
    /// The core hint, verbatim.
    pub hint: Option<String>,
    /// The canonical text for a bug report, copied by `y`.
    pub full: String,
    /// What `r` runs again. Hidden for misuse and unsafe names.
    pub retry: Option<Retry>,
}

/// A confirmation living two seconds in the lower right corner.
#[derive(Debug, Clone)]
pub struct Toast {
    /// The text.
    pub text: String,
    /// When the toast was raised.
    born: Instant,
}

/// Live transfer state shown in the progress panel, either direction.
#[derive(Debug)]
pub struct Transfer {
    /// The direction: the panel, the fold and the statuses branch on it.
    pub dir: Dir,
    /// Transfer: the repository URL the names are relative to.
    /// Upload: the remote base URL the folder uploads into.
    pub repo_url: String,
    /// The server label, for error facts.
    pub server: Option<String>,
    /// The repository name, for error facts.
    pub repo_name: Option<String>,
    /// Transfer: the local target directory. Empty for an upload.
    pub dst: PathBuf,
    /// Upload: the local source directory.
    /// None for a download.
    pub src: Option<PathBuf>,
    /// The plan count of the walk, shown before the transfer starts.
    pub files: Option<usize>,
    /// `(to transfer, skipped)` from the plan event.
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
    /// The walk scope: the path from the repository root, empty for the whole.
    pub scope: String,
    /// True when the collected names are stripped to the scope, so they land
    /// straight under `{dst}` (the dialog's `{dir}/{name}` target).
    pub scope_relative: bool,
    /// The task of the running transfer: `x` aborts through it.
    pub handle: Option<tokio::task::JoinHandle<()>>,
    /// Set by `x`: the stragglers of the aborted task are ignored.
    pub cancelled: bool,
    /// What `r` in a failure modal runs again.
    pub retry: Option<Retry>,
}

impl Default for Transfer {
    /// A download: the historical direction of the panel.
    fn default() -> Self {
        Self {
            dir: Dir::Down,
            repo_url: String::new(),
            server: None,
            repo_name: None,
            dst: PathBuf::new(),
            src: None,
            files: None,
            planned: None,
            active: BTreeMap::new(),
            finished: 0,
            skipped: 0,
            note: None,
            outcome: None,
            scope: String::new(),
            scope_relative: false,
            handle: None,
            cancelled: false,
            retry: None,
        }
    }
}

impl Transfer {
    /// True until the final result arrives.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.outcome.is_none()
    }
}

/// Transfer progress events folded from the core event stream.
#[derive(Debug)]
pub enum DlEv {
    /// The transfer is set up: the local side (destination or source) and the
    /// plan count when the walk already knows it.
    Start {
        /// Transfer: the destination folder. Upload: the source folder.
        local: PathBuf,
        /// The plan count, when it is known before the plan event.
        files: Option<usize>,
    },
    /// The diff plan: `(to transfer, skipped)`.
    Plan { transfer: usize, skip: usize },
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
    /// The destination picker: `o`, or a download waiting for a folder.
    Dest(DestPicker),
    /// The source picker of a put: a local folder to upload.
    PickSource(PickSource),
    /// The card over a repository or a file.
    Card(Card),
    /// The download-as dialog.
    SaveAs(SaveAsDialog),
    /// The error modal, with the dialog layer it covered: restored on close.
    Error {
        /// The modal itself: cause, facts, hint, full text, retry.
        modal: ErrorModal,
        /// The dialog the modal covered. `None` over the normal screen:
        /// errors never restore a plain browse.
        back: Option<Box<Mode>>,
    },
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
    /// The HEAD result behind a file card, `rel` being the row token.
    Head {
        tab: usize,
        gen: u64,
        rel: String,
        res: Result<HeadInfo, Error>,
    },
    /// The `.sha256` sibling result behind a file card, `rel` being the row token.
    Sibling {
        tab: usize,
        gen: u64,
        rel: String,
        res: Result<Option<String>, Error>,
    },
    /// The 500 ms tick: toasts expire, card placeholders refresh.
    Tick,
    /// A download progress event.
    Dl(DlEv),
    /// The listing of one local folder behind the dual-pane. `cwd` is the
    /// row token: a stale answer of another folder is dropped.
    LocalEntries {
        cwd: PathBuf,
        res: Result<Vec<LocalEntry>, Error>,
    },
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
    /// The download directory from the config: the seed of the session destination.
    pub download_dir: PathBuf,
    /// The folder downloads land in this session: `{dst}/{name}`.
    pub destination: PathBuf,
    /// True once the user confirmed the destination with `o`, the picker or a dialog.
    pub dest_confirmed: bool,
    /// The working directory at launch: relative picker input resolves against it.
    pub base_dir: PathBuf,
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
    /// True while a connect was started from a live form: when the user
    /// cancels the form, the result no longer opens a tab or writes the config.
    connect_open: bool,
    /// Loads, walks and HEADs in flight, across every tab: a diagnostic count,
    /// not a gate (the gates are the per-tab `loading` and the running download).
    pub pending: usize,
    /// The current or last transfer, either direction, if any.
    pub transfer: Option<Transfer>,
    /// The local folder the last confirmed put uploaded from: the seed of
    /// the source picker.
    /// A session value, never written to the config.
    pub upload_source: Option<PathBuf>,
    /// The plain status line.
    pub status: String,
    /// The toasts on screen, oldest first.
    pub toasts: VecDeque<Toast>,
    /// Set when the loop should stop.
    pub quit: bool,
    /// Set after the first `q` while a download is running: the second `q` quits.
    pub quit_armed: bool,
    /// The render state of the list: kept across frames so the scroll offset
    /// survives and mouse clicks can hit-test.
    pub list_state: ListState,
    /// The inner area of the list block, set by every frame of [`crate::ui::draw`].
    pub list_area: Rect,
    /// The local pane of dual-pane browsing, when open.
    pub local: Option<LocalPane>,
    /// True when the keyboard sits in the local pane.
    pub local_focus: bool,
    /// The render state of the local pane: kept across frames so the scroll
    /// offset survives and mouse clicks can hit-test.
    pub local_state: ListState,
    /// The inner area of the local pane block, set by every frame.
    pub local_area: Rect,
    /// The terminal area, set by every frame of [`crate::ui::draw`]: the
    /// dual-pane gate measures it.
    pub term: Rect,
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
        let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            servers,
            auth,
            all_formats,
            nav,
            destination: download_dir.clone(),
            dest_confirmed: false,
            base_dir,
            config,
            config_path,
            tabs,
            tab: 0,
            mode: Mode::Normal,
            servers_cursor: 0,
            connecting: 0,
            connect_open: false,
            pending: 0,
            transfer: None,
            upload_source: None,
            status: String::new(),
            toasts: VecDeque::new(),
            quit: false,
            quit_armed: false,
            list_state: ListState::default(),
            list_area: Rect::default(),
            local: None,
            local_focus: false,
            local_state: ListState::default(),
            local_area: Rect::default(),
            term: Rect::default(),
            last_click: None,
            download_dir,
            tx,
        }
    }

    /// Fills the first status line: the screens are already populated, nothing is in flight.
    pub fn boot(&mut self) {
        let repos: usize = self.tabs.iter().map(|t| t.repos.len()).sum();
        self.status = format!(
            "{repos} repositories on {} server(s), enter opens a raw repository, d downloads it",
            self.tabs.len()
        );
    }

    /// True while the 500 ms tick has something to do: a live toast, or a
    /// card with metadata still in flight. The rest of the time the pump
    /// pays nothing for the tick.
    #[must_use]
    pub fn needs_tick(&self) -> bool {
        if !self.toasts.is_empty() {
            return true;
        }
        matches!(
            self.mode.clone(),
            Mode::Card(Card::File(c)) if c.size == SizeState::Pending || c.sha == ShaState::Pending
        )
    }

    /// Drops expired toasts. Called on every tick.
    pub fn tick(&mut self) {
        self.toasts.retain(|t| t.born.elapsed() < TOAST_TTL);
    }

    /// Raises a toast: the last [`TOAST_STACK`] survive, the oldest dies first.
    fn toast(&mut self, text: &str) {
        self.toasts.push_back(Toast {
            text: text.to_owned(),
            born: Instant::now(),
        });
        while self.toasts.len() > TOAST_STACK {
            self.toasts.pop_front();
        }
    }

    /// Opens the error modal over whatever is on screen: errors never live in
    /// the status line.
    fn raise_error(&mut self, kind: ErrKind, e: &Error, ctx: ErrCtx, retry: Option<Retry>) {
        let mut facts: Vec<(String, String)> = Vec::new();
        if let Some(server) = &ctx.server {
            facts.push(("server".into(), server.escape_debug().to_string()));
        }
        if let Some(repository) = &ctx.repository {
            facts.push(("repository".into(), repository.escape_debug().to_string()));
        }
        if let Some(destination) = &ctx.destination {
            facts.push(("destination".into(), destination.display().to_string()));
        }
        if let Some(source) = &ctx.source {
            facts.push(("source".into(), source.display().to_string()));
        }
        let http = match e {
            Error::Http { status, .. } | Error::ReadOnly { status, .. } => Some(*status),
            Error::ServiceMissing { .. } => Some(404),
            _ => None,
        };
        if let Some(status) = http {
            facts.push(("http status".into(), status.to_string()));
        }
        match e {
            Error::Mismatch { name, .. } => {
                facts.push(("name".into(), name.escape_debug().to_string()));
            }
            Error::UnsafeName { name, .. } => {
                facts.push(("name".into(), name.escape_debug().to_string()));
            }
            Error::Io { path, .. } => {
                facts.push(("path".into(), path.escape_debug().to_string()));
            }
            Error::Incomplete { names } | Error::Missing { names } if !names.is_empty() => {
                facts.push((
                    "names".into(),
                    names
                        .iter()
                        .map(|n| n.escape_debug().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            _ => {}
        }
        let hint = e.hint();
        let full = match &hint {
            Some(h) => format!("error: {e}\nhint: {h}"),
            None => format!("error: {e}"),
        };
        // A background failure never kills a dialog: the layer comes back
        // when the modal closes. The normal screen is not worth remembering.
        let back = match self.mode.clone() {
            Mode::Dest(_)
            | Mode::PickSource(_)
            | Mode::SaveAs(_)
            | Mode::AddServer(_)
            | Mode::Servers
            | Mode::Card(_) => Some(Box::new(self.mode.clone())),
            // An error over an error hands the covered layer on.
            Mode::Error {
                back: Some(back), ..
            } => Some(back),
            _ => None,
        };
        self.mode = Mode::Error {
            modal: ErrorModal {
                title: kind.title(e),
                cause: cause_of(e),
                facts,
                hint,
                full,
                retry,
            },
            back,
        };
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
            Msg::Head { tab, gen, rel, res } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_head(tab, gen, rel, res);
            }
            Msg::Sibling { tab, gen, rel, res } => {
                self.pending = self.pending.saturating_sub(1);
                self.on_sibling(tab, gen, rel, res);
            }
            Msg::Tick => self.tick(),
            Msg::Dl(ev) => self.on_dl(ev),
            Msg::LocalEntries { cwd, res } => self.on_local_entries(cwd, res),
        }
    }

    // ---- input -----------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        // Any key but the arming one disarms the quit guard.
        let is_q = pressed(&key, 'q');
        if !is_q {
            self.quit_armed = false;
        }
        match self.mode.clone() {
            Mode::Normal => self.on_key_normal(key),
            Mode::Help => self.on_key_help(key),
            Mode::Servers => self.on_key_servers(key),
            Mode::AddServer(buffer) => self.on_key_add(key, buffer),
            Mode::Dest(picker) => self.on_key_dest(key, picker),
            Mode::PickSource(picker) => self.on_key_source(key, picker),
            Mode::Card(card) => self.on_key_card(key, card),
            Mode::SaveAs(dialog) => self.on_key_save_as(key, dialog),
            Mode::Error { modal, back } => self.on_key_error(key, modal, back),
        }
    }

    fn on_key_normal(&mut self, key: KeyEvent) {
        let editing = self.tabs.get(self.tab).is_some_and(|t| t.filter_edit);
        if editing {
            self.on_key_filter(key);
            return;
        }
        // The toggle works from both panes.
        if pressed(&key, 'v') {
            self.toggle_local_pane();
            return;
        }
        // The local pane eats the keys while it holds the focus.
        if self.local_focus && self.local.is_some() {
            self.on_key_local(key);
            return;
        }
        match key.code {
            KeyCode::Char(_) if pressed(&key, 'q') => self.request_quit(),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Left => self.on_left(),
            KeyCode::Right => self.on_right(),
            KeyCode::Char(_) if pressed(&key, 'k') => self.move_cursor(-1),
            KeyCode::Char(_) if pressed(&key, 'j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_page(-1),
            KeyCode::PageDown => self.move_page(1),
            KeyCode::Home => self.cursor_edge(0),
            KeyCode::Char(_) if pressed(&key, 'g') => self.cursor_edge(0),
            KeyCode::End => self.cursor_edge(1),
            KeyCode::Char(_) if pressed(&key, 'G') => self.cursor_edge(1),
            KeyCode::Enter => self.on_enter(),
            KeyCode::Esc | KeyCode::Backspace => self.on_back(),
            KeyCode::Char(_) if pressed(&key, 'o') => self.open_dest_picker(None, None),
            KeyCode::Char(_) if pressed(&key, 'd') => self.quick_download(),
            KeyCode::Char(_) if pressed(&key, 'p') => self.open_source_picker(),
            KeyCode::Char(_) if pressed(&key, 'x') => self.cancel_transfer(),
            KeyCode::Char(_) if pressed(&key, 'c') => self.copy_selected_url(),
            KeyCode::Char(_) if pressed(&key, 'h') => self.focus_local_pane(),
            KeyCode::Char(_) if pressed(&key, 'D') => self.open_save_as(),
            KeyCode::Char(_) if pressed(&key, 'r') => self.refresh(),
            KeyCode::Char(_) if pressed(&key, 'e') => self.toggle_nav(),
            KeyCode::Char(_) if pressed(&key, 'i') => self.open_card(),
            KeyCode::Char(_) if pressed(&key, 's') => self.open_servers_overlay(),
            KeyCode::Char(_) if pressed(&key, '?') => self.mode = Mode::Help,
            KeyCode::Char(_) if pressed(&key, '/') => self.start_filter(),
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(-1),
            KeyCode::Char(c) if key.modifiers.is_empty() && c.is_ascii_digit() && c != '0' => {
                self.jump_tab(c.to_digit(10).unwrap_or(0) as usize - 1);
            }
            _ => {}
        }
    }

    /// The filter input eats every key while it is open: characters extend
    /// the live pattern, backspace shortens it, esc clears it, enter applies
    /// and closes. An empty pattern is off: backspace to empty closes the
    /// input instead of leaving a zombie filter.
    fn on_key_filter(&mut self, key: KeyEvent) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        if !t.filter_edit {
            return;
        }
        match key.code {
            KeyCode::Esc => {
                t.filter_edit = false;
                t.filter = None;
                t.tree_cursor = 0;
                self.status = hint_tree();
                return;
            }
            KeyCode::Enter => {
                t.filter_edit = false;
                if t.filter.as_deref().is_none_or(str::is_empty) {
                    t.filter = None;
                    self.status = hint_tree();
                } else {
                    let shown = t.row_idx().len();
                    let total = t.rows().len();
                    self.status = format!("filter kept: {shown} of {total}");
                }
                return;
            }
            KeyCode::Backspace => {
                if let Some(f) = t.filter.as_mut() {
                    f.pop();
                }
                t.tree_cursor = 0;
            }
            KeyCode::Char(c) if text_char(&key) == Some(c) => {
                if let Some(f) = t.filter.as_mut() {
                    f.push(c);
                }
                t.tree_cursor = 0;
            }
            _ => return,
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        self.status = match &t.filter {
            Some(f) if !f.is_empty() => format!(
                "filter \"{f}\": {} of {} shown",
                t.row_idx().len(),
                t.rows().len()
            ),
            _ => {
                // Backspace to empty: the mode closes, the filter is off.
                if let Some(t) = self.tabs.get_mut(self.tab) {
                    t.filter = None;
                    t.filter_edit = false;
                }
                hint_tree()
            }
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
            // q closes the overlay: quitting the app is a Normal-screen act.
            KeyCode::Esc => {
                self.detach_connect();
                self.mode = Mode::Normal;
            }
            KeyCode::Char(_) if pressed(&key, 'q') => {
                self.detach_connect();
                self.mode = Mode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.servers_cursor = self.servers_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.servers_cursor =
                    (self.servers_cursor + 1).min(self.servers.len().saturating_sub(1));
            }
            KeyCode::Enter => self.open_server(self.servers_cursor),
            KeyCode::Char(_) if pressed(&key, 'a') => {
                self.mode = Mode::AddServer(String::new());
            }
            _ => {}
        }
    }

    fn on_key_add(&mut self, key: KeyEvent, mut buffer: String) {
        match key.code {
            // One layer: back to the servers overlay, not straight to Normal.
            // A connect already flying is detached: no tab, no config write.
            KeyCode::Esc => {
                self.detach_connect();
                self.mode = Mode::Servers;
            }
            KeyCode::Enter => {
                self.mode = Mode::AddServer(buffer.clone());
                self.submit_server(buffer);
            }
            KeyCode::Backspace => {
                buffer.pop();
                self.mode = Mode::AddServer(buffer);
            }
            KeyCode::Char(c) if text_char(&key) == Some(c) => {
                buffer.push(c);
                self.mode = Mode::AddServer(buffer);
            }
            _ => {}
        }
    }

    /// The destination picker: enter confirms, esc cancels, typing edits.
    fn on_key_dest(&mut self, key: KeyEvent, mut picker: DestPicker) {
        match key.code {
            KeyCode::Esc => self.close_dest_picker(picker),
            KeyCode::Enter => self.confirm_destination(picker),
            KeyCode::Backspace => {
                picker.buffer.pop();
                self.mode = Mode::Dest(picker);
            }
            KeyCode::Char(c) if text_char(&key) == Some(c) => {
                picker.buffer.push(c);
                self.mode = Mode::Dest(picker);
            }
            _ => {}
        }
    }

    /// The source picker: enter uploads, esc cancels, typing edits.
    fn on_key_source(&mut self, key: KeyEvent, mut picker: PickSource) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => self.confirm_source(picker),
            KeyCode::Backspace => {
                picker.buffer.pop();
                self.mode = Mode::PickSource(picker);
            }
            KeyCode::Char(c) if text_char(&key) == Some(c) => {
                picker.buffer.push(c);
                self.mode = Mode::PickSource(picker);
            }
            _ => {}
        }
    }

    /// The card: inspect, download, recopy, or leave.
    fn on_key_card(&mut self, key: KeyEvent, card: Card) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char(_) if pressed(&key, 'q') => {
                self.mode = Mode::Normal;
            }
            KeyCode::Char(_) if pressed(&key, 'c') => {
                let url = card.url().to_owned();
                clipboard::copy(&url, self.config.tui.osc52);
                self.toast("copied");
            }
            KeyCode::Char(_) if pressed(&key, 's') => self.card_download(card),
            KeyCode::Char(_) if pressed(&key, 'o') => {
                self.open_dest_picker(Some(card), None);
            }
            _ => {}
        }
    }

    /// The download-as dialog: two fields, enter walks them, esc cancels.
    fn on_key_save_as(&mut self, key: KeyEvent, mut dialog: SaveAsDialog) {
        match key.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Tab | KeyCode::Down | KeyCode::Up => {
                dialog.field = match dialog.field {
                    SaveAsField::Dir => SaveAsField::Name,
                    SaveAsField::Name => SaveAsField::Dir,
                };
                self.mode = Mode::SaveAs(dialog);
            }
            KeyCode::Enter => self.confirm_save_as(dialog),
            KeyCode::Backspace => {
                match dialog.field {
                    SaveAsField::Dir => {
                        dialog.dir.pop();
                    }
                    SaveAsField::Name => {
                        dialog.name.pop();
                    }
                }
                self.mode = Mode::SaveAs(dialog);
            }
            KeyCode::Char(c) if text_char(&key) == Some(c) => {
                match dialog.field {
                    SaveAsField::Dir => dialog.dir.push(c),
                    SaveAsField::Name => dialog.name.push(c),
                }
                self.mode = Mode::SaveAs(dialog);
            }
            _ => {}
        }
    }

    /// The error modal: esc leaves to the covered layer, `y` copies the full
    /// text, `r` reruns.
    /// A remembered dialog gets its layer back on close.
    fn on_key_error(&mut self, key: KeyEvent, modal: ErrorModal, back: Option<Box<Mode>>) {
        let restore = || match back {
            Some(layer) => *layer,
            None => Mode::Normal,
        };
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                self.mode = restore();
            }
            KeyCode::Char(_) if pressed(&key, 'q') => {
                self.mode = restore();
            }
            KeyCode::Char(_) if pressed(&key, 'y') => {
                clipboard::copy(&modal.full, self.config.tui.osc52);
                self.toast("copied");
            }
            KeyCode::Char(_) if pressed(&key, 'r') => {
                if let Some(retry) = modal.retry {
                    self.mode = restore();
                    self.run_retry(retry);
                }
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        let clean: String = text.chars().filter(|c| !c.is_control()).collect();
        match self.mode.clone() {
            Mode::AddServer(mut buffer) => {
                buffer.push_str(&clean);
                self.mode = Mode::AddServer(buffer);
            }
            Mode::Dest(mut picker) => {
                picker.buffer.push_str(&clean);
                self.mode = Mode::Dest(picker);
            }
            Mode::PickSource(mut picker) => {
                picker.buffer.push_str(&clean);
                self.mode = Mode::PickSource(picker);
            }
            Mode::SaveAs(mut dialog) => {
                match dialog.field {
                    SaveAsField::Dir => dialog.dir.push_str(&clean),
                    SaveAsField::Name => dialog.name.push_str(&clean),
                }
                self.mode = Mode::SaveAs(dialog);
            }
            _ => {
                if let Some(t) = self.tabs.get_mut(self.tab) {
                    if t.filter_edit {
                        if let Some(filter) = t.filter.as_mut() {
                            filter.push_str(&clean);
                        }
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
            MouseEventKind::Down(MouseButton::Left) => self.on_click(mouse.column, mouse.row),
            _ => {}
        }
    }

    /// A left click selects the row under the pointer; a second click on the
    /// same row within [`DOUBLE_CLICK`] activates it.
    /// With the local pane open, the left half selects in the pane and the
    /// right half in the list.
    fn on_click(&mut self, col: u16, row: u16) {
        if self.mode != Mode::Normal {
            return;
        }
        if let Some(pane) = self.local.as_ref() {
            let area = self.local_area;
            if area.width > 0
                && col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
            {
                let idx = self.local_state.offset() + (row - area.y) as usize;
                if idx < pane.entries.len() {
                    let pane = self.local.as_mut().expect("pane checked above");
                    pane.cursor = idx;
                    self.local_focus = true;
                }
                return;
            }
        }
        if self.list_area.width == 0 {
            return;
        }
        if col < self.list_area.x {
            return;
        }
        // The focus follows the click: the right half is the remote pane.
        self.local_focus = false;
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
    fn transfer_busy(&self) -> bool {
        self.transfer.as_ref().is_some_and(Transfer::is_running)
    }

    /// The `x` verb: abort the running transfer. There is no cancel modal:
    /// restarting is a normal `d` or `p`.
    fn cancel_transfer(&mut self) {
        let Some(dl) = self.transfer.as_mut() else {
            self.status = "nothing is running".into();
            return;
        };
        if !dl.is_running() {
            self.status = "nothing is running".into();
            return;
        }
        if let Some(handle) = dl.handle.take() {
            handle.abort();
        }
        dl.cancelled = true;
        dl.active.clear();
        dl.outcome = Some(DlOutcome::Cancelled);
        self.status = "cancelled".into();
    }

    /// The `c` verb: copy the URL of the selection. The repository on the
    /// repositories screen, the file or folder on the tree.
    fn copy_selected_url(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let url = match t.screen {
            Screen::Repos => t.repos.get(t.repos_cursor).map(|r| r.url.clone()),
            Screen::Tree => t
                .selected_row()
                .map(|row| format!("{}{}", t.here_url(), row.rel)),
        };
        let Some(url) = url else {
            return;
        };
        clipboard::copy(&url, self.config.tui.osc52);
        self.toast("copied");
    }

    fn request_quit(&mut self) {
        if self.transfer.as_ref().is_some_and(Transfer::is_running) && !self.quit_armed {
            self.quit_armed = true;
            self.status = "transfer in flight: press q again to quit".into();
            return;
        }
        self.quit = true;
    }

    fn on_enter(&mut self) {
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
                    // The primary verb inspects: downloading is `d` or the card's `s`.
                    EntryKind::File => self.open_file_card(t.path.clone(), row),
                    EntryKind::Dir => {
                        if self.listing_busy() {
                            self.status = "a listing is in flight".into();
                            return;
                        }
                        self.descend(row.name);
                    }
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
        t.filter_edit = false;
        t.keep_cursor_name = None;
        t.tree_cursor = 0;
        t.gen += 1;
        t.loading = true;
        self.pending += 1;
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
        t.filter_edit = false;
        t.keep_cursor_name = None;
        t.tree_cursor = 0;
        t.gen += 1;
        t.loading = true;
        self.pending += 1;
        let (tab, gen) = (self.tab, t.gen);
        net::load_entries(self.tx.clone(), tab, gen, url, self.auth.clone());
    }

    fn on_back(&mut self) {
        let Some(t) = self.tabs.get_mut(self.tab) else {
            return;
        };
        // Listings of an abandoned drill-down are dropped when they arrive.
        t.gen += 1;
        match t.screen {
            Screen::Repos => self.status = "press q to quit".into(),
            Screen::Tree if t.path.is_empty() => {
                // The tab returns virgin: no hidden repository, no hidden mine.
                t.screen = Screen::Repos;
                t.repo = None;
                t.path.clear();
                t.entries.clear();
                t.clear_expansion();
                t.filter = None;
                t.filter_edit = false;
                t.keep_cursor_name = None;
                t.tree_cursor = 0;
                t.loading = false;
                self.status = hint_repos();
            }
            Screen::Tree => {
                t.path.pop();
                let url = t.here_url();
                t.entries.clear();
                t.filter = None;
                t.filter_edit = false;
                t.keep_cursor_name = None;
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
        // Reopening the input keeps the applied pattern for editing.
        if t.filter.is_none() {
            t.filter = Some(String::new());
        }
        t.filter_edit = true;
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
                        // Both verbs of a file inspect now: no refusal class.
                        Some(row) if row.kind == EntryKind::File => {
                            self.open_file_card(t.path.clone(), row);
                        }
                        Some(_) => {}
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
        self.refresh_tab_inner(self.tab, false);
    }

    /// Refreshes one tab's current screen. The applied filter survives and the
    /// cursor returns to the row it sat on, by name: `r` rereads a listing
    /// without throwing the user's place away.
    fn refresh_tab(&mut self, idx: usize) {
        self.refresh_tab_inner(idx, false);
    }

    /// The quiet refresh of a finished put: no status lines, the uploaded
    /// summary stays on screen.
    fn quiet_refresh_at(&mut self, base: &str) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if t.loading || t.here_url() != base {
            return;
        }
        self.refresh_tab_inner(self.tab, true);
    }

    fn refresh_tab_inner(&mut self, idx: usize, quiet: bool) {
        if idx >= self.tabs.len() {
            return;
        }
        let keep = {
            let t = &self.tabs[idx];
            if t.loading {
                if idx == self.tab && !quiet {
                    self.status = "a listing is in flight".into();
                }
                return;
            }
            match t.screen {
                Screen::Repos => t.repos.get(t.repos_cursor).map(|r| r.name.clone()),
                Screen::Tree => t.selected_row().map(|r| r.name),
            }
        };
        let Some(t) = self.tabs.get_mut(idx) else {
            return;
        };
        t.keep_cursor_name = keep;
        t.refresh_quiet = quiet;
        t.gen += 1;
        // A refresh redraws the current listing: the inline expansion folds too.
        t.clear_expansion();
        match t.screen {
            Screen::Repos => {
                t.loading = true;
                self.pending += 1;
                if idx == self.tab && !quiet {
                    self.status = format!("refreshing {}", t.server.name);
                }
                let server = t.server.clone();
                let (slot, auth) = (idx, self.auth.clone());
                net::load_repos(self.tx.clone(), server, slot, false, auth);
            }
            Screen::Tree => {
                let url = t.here_url();
                t.loading = true;
                self.pending += 1;
                if idx == self.tab && !quiet {
                    self.status = format!("refreshing {}/", t.breadcrumb());
                }
                let (tab, gen, auth) = (idx, t.gen, self.auth.clone());
                net::load_entries(self.tx.clone(), tab, gen, url, auth);
            }
        }
    }

    /// The `i` action on Repos: the repository card. On Tree the key is cut:
    /// the Enter card covers files, the title line covers folders.
    fn open_card(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if t.screen != Screen::Repos {
            return;
        }
        let Some(repo) = t.repos.get(t.repos_cursor) else {
            return;
        };
        let card = RepoCard {
            repo: RepoRow::from(repo),
            enterable: t.enterable(repo, self.all_formats),
        };
        self.mode = Mode::Card(Card::Repo(card));
    }

    /// Opens the file card: the metadata HEADs in the background while the
    /// card shows placeholders. The row token (`tab`, `gen`, `rel`) guards the
    /// answers: a late HEAD of another row never paints over this card.
    fn open_file_card(&mut self, path: Vec<String>, row: Row) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let Some(repo) = t.repo.clone() else {
            return;
        };
        let rel = if path.is_empty() {
            row.rel.clone()
        } else {
            format!("{}/{}", path.join("/"), row.rel)
        };
        let url = format!("{}{}", t.here_url(), row.rel);
        let card = FileCard {
            tab: self.tab,
            gen: t.gen,
            name: row.name.clone(),
            rel: rel.clone(),
            url: url.clone(),
            repo: RepoRow::from(&repo),
            size: SizeState::Pending,
            sha: ShaState::Pending,
        };
        self.mode = Mode::Card(Card::File(Box::new(card)));
        self.pending += 2;
        let (tab, gen, auth) = (self.tab, t.gen, self.auth.clone());
        net::head_size(self.tx.clone(), tab, gen, rel.clone(), url.clone(), auth);
        net::fetch_sibling(
            self.tx.clone(),
            tab,
            gen,
            rel,
            format!("{url}.sha256"),
            self.auth.clone(),
        );
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
        self.connect_open = true;
        self.pending += 1;
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

    // ---- destination -----------------------------------------------------

    /// Opens the destination picker. `pending_repo` defers a repository
    /// download until the folder is confirmed, `back_to_card` returns into a
    /// card instead of the normal screen.
    fn open_dest_picker(&mut self, back_to_card: Option<Card>, pending_repo: Option<RepoRow>) {
        let picker = DestPicker {
            buffer: self.destination.display().to_string(),
            back_to_card,
            pending_repo,
        };
        self.mode = Mode::Dest(picker);
    }

    /// Cancels the picker: nothing changes, the deferred download dies.
    fn close_dest_picker(&mut self, picker: DestPicker) {
        self.mode = match picker.back_to_card {
            Some(card) => Mode::Card(card),
            None => Mode::Normal,
        };
    }

    /// Confirms the picker: the session destination is set, the deferred
    /// repository download starts, a card gets its layer back.
    fn confirm_destination(&mut self, picker: DestPicker) {
        let raw = picker.buffer.trim().to_owned();
        if raw.is_empty() {
            self.status = "the destination is empty".into();
            return;
        }
        self.destination = self.resolve_path(Path::new(&raw));
        self.dest_confirmed = true;
        if let Some(repo) = picker.pending_repo {
            self.mode = Mode::Normal;
            self.status = format!("destination: {}", self.destination.display());
            self.download_repo(&repo);
            return;
        }
        self.status = format!("destination: {}", self.destination.display());
        self.mode = match picker.back_to_card {
            Some(card) => Mode::Card(card),
            None => Mode::Normal,
        };
    }

    /// Absolute stays, relative resolves against the working directory at
    /// launch. Tilde expansion is out of scope for this session.
    #[must_use]
    pub fn resolve_path(&self, raw: &Path) -> PathBuf {
        if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base_dir.join(raw)
        }
    }

    // ---- uploads ---------------------------------------------------------

    /// The `p` verb: put a local folder into the current tree position.
    /// A running transfer waits, the repositories screen refuses.
    fn open_source_picker(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if t.screen != Screen::Tree {
            self.status = "the put works in the tree".into();
            return;
        }
        if self.transfer_busy() {
            self.status = "transfer in flight: one at a time".into();
            return;
        }
        let seed = match self.local.as_ref() {
            // The pane anchors the put too: its folder prefills the picker.
            Some(pane) => pane.cwd.clone(),
            None => self
                .upload_source
                .clone()
                .unwrap_or_else(|| self.base_dir.clone()),
        };
        self.mode = Mode::PickSource(PickSource {
            buffer: seed.display().to_string(),
        });
    }

    /// The Enter of the source picker: the folder is checked locally, nothing
    /// leaves the machine before it. A refused path keeps the picker open.
    fn confirm_source(&mut self, picker: PickSource) {
        let raw = picker.buffer.trim().to_owned();
        if raw.is_empty() {
            self.status = "the source is empty".into();
            return;
        }
        let src = self.resolve_path(Path::new(&raw));
        if !src.is_dir() {
            self.status = format!("not a directory: {}", src.display());
            return;
        }
        self.upload_source = Some(src.clone());
        self.mode = Mode::Normal;
        let base = self
            .tabs
            .get(self.tab)
            .map(|t| t.here_url())
            .unwrap_or_default();
        self.start_put(base, src);
    }

    /// Uploads `src` into the remote `base`: the slot is taken, the position
    /// is fixed at start, navigation never retargets the run.
    fn start_put(&mut self, base: String, src: PathBuf) {
        if self.transfer_busy() {
            self.status = "transfer in flight: one at a time".into();
            return;
        }
        let (server, repo_name, pos) = match self.tabs.get(self.tab) {
            Some(t) => (
                Some(t.server.name.clone()),
                t.repo.as_ref().map(|r| r.name.clone()),
                t.breadcrumb(),
            ),
            None => (None, None, String::new()),
        };
        self.transfer = Some(Transfer {
            dir: Dir::Up,
            repo_url: base.clone(),
            server,
            repo_name,
            src: Some(src.clone()),
            retry: Some(Retry::Put {
                src: src.clone(),
                base: base.clone(),
            }),
            ..Transfer::default()
        });
        self.status = format!("put {} -> {}", src.display(), pos);
        let handle = net::start_upload(self.tx.clone(), base, src, self.auth.clone());
        if let Some(dl) = self.transfer.as_mut() {
            dl.handle = Some(handle);
        }
    }

    // ---- local pane ------------------------------------------------------

    /// The `v` toggle: the local pane joins the screen, or leaves it. The
    /// anchors `d` and `p` follow the pane while it is open.
    fn toggle_local_pane(&mut self) {
        if self.local.is_some() {
            self.local = None;
            self.local_focus = false;
            self.status = "dual-pane off".into();
            return;
        }
        if self.term.width < MIN_DUAL_COLS {
            self.status = "the terminal is too narrow for dual-pane".into();
            return;
        }
        let cwd = self
            .upload_source
            .clone()
            .unwrap_or_else(|| self.base_dir.clone());
        self.local = Some(LocalPane {
            cwd,
            entries: Vec::new(),
            cursor: 0,
            loading: true,
        });
        self.local_focus = true;
        self.status = "dual-pane on".into();
        self.list_local();
    }

    /// The `h` key: the focus moves to the local pane, when one is open.
    fn focus_local_pane(&mut self) {
        if self.local.is_some() {
            self.local_focus = true;
        }
    }

    /// Lists the pane folder in the background.
    fn list_local(&mut self) {
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        pane.loading = true;
        let cwd = pane.cwd.clone();
        net::local_entries(self.tx.clone(), cwd);
    }

    /// The keys of the focused local pane: navigation, enter, back, and the
    /// refusals of the verbs that work on the remote side only.
    fn on_key_local(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(_) if pressed(&key, 'q') => self.request_quit(),
            KeyCode::Char(_) if pressed(&key, 'l') => {
                self.local_focus = false;
            }
            KeyCode::Up | KeyCode::Char('k') => self.local_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.local_move(1),
            KeyCode::PageUp => self.local_move(-self.local_page()),
            KeyCode::PageDown => self.local_move(self.local_page()),
            KeyCode::Home | KeyCode::Char('g') => self.local_edge(0),
            KeyCode::End | KeyCode::Char('G') => self.local_edge(1),
            KeyCode::Enter => self.local_descend(),
            KeyCode::Esc | KeyCode::Backspace => self.local_up(),
            KeyCode::Char(_) if pressed(&key, 'd') || pressed(&key, 'p') => {
                self.status = "switch to the remote pane (l)".into();
            }
            KeyCode::Char('/') => self.status = "the filter works in the tree".into(),
            _ => {}
        }
    }

    /// One page of the local pane, in rows.
    fn local_page(&self) -> i32 {
        self.local_area.height.max(1).saturating_sub(1) as i32
    }

    fn local_move(&mut self, delta: i32) {
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        let len = pane.entries.len();
        if len == 0 {
            return;
        }
        pane.cursor = ((pane.cursor as i64) + delta as i64).clamp(0, len as i64 - 1) as usize;
    }

    fn local_edge(&mut self, edge: u8) {
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        let len = pane.entries.len();
        if len == 0 {
            return;
        }
        pane.cursor = if edge == 0 { 0 } else { len - 1 };
    }

    /// Enter on a folder descends, on a file it reports: the local card does
    /// not exist in this wave.
    fn local_descend(&mut self) {
        let entry = self
            .local
            .as_ref()
            .and_then(|pane| pane.entries.get(pane.cursor))
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        if entry.kind != EntryKind::Dir {
            self.status = "enter opens folders here".into();
            return;
        }
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        pane.cwd = pane.cwd.join(&entry.name);
        pane.entries.clear();
        pane.cursor = 0;
        self.list_local();
    }

    /// Backspace or esc: one folder up, the filesystem root reports.
    fn local_up(&mut self) {
        let parent = self
            .local
            .as_ref()
            .and_then(|pane| pane.cwd.parent())
            .map(Path::to_path_buf);
        let Some(parent) = parent else {
            self.status = "at the root of the filesystem".into();
            return;
        };
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        pane.cwd = parent;
        pane.entries.clear();
        pane.cursor = 0;
        self.list_local();
    }

    /// The listing of one pane folder lands: stale answers of other folders drop.
    fn on_local_entries(&mut self, cwd: PathBuf, res: Result<Vec<LocalEntry>, Error>) {
        let Some(pane) = self.local.as_mut() else {
            return;
        };
        if pane.cwd != cwd {
            return;
        }
        pane.loading = false;
        match res {
            Ok(mut entries) => {
                entries.sort_by_key(|e| (e.kind != EntryKind::Dir, e.name.clone()));
                let len = entries.len();
                pane.entries = entries;
                pane.cursor = pane.cursor.min(len.saturating_sub(1));
            }
            Err(e) => {
                self.raise_error(ErrKind::Listing, &e, ErrCtx::default(), None);
            }
        }
    }

    /// The folder transfers anchor to while the pane is open: its folder.
    /// The session destination itself is never written by a pane-anchored
    /// transfer.
    fn anchor_base(&self) -> PathBuf {
        match self.local.as_ref() {
            Some(pane) => pane.cwd.clone(),
            None => self.destination.clone(),
        }
    }

    // ---- downloads -------------------------------------------------------

    /// The `d` verb: on Repos the whole repository of the row, on Tree the
    /// selected entry. An unconfirmed destination is picked first, once.
    fn quick_download(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        match t.screen {
            Screen::Repos => {
                let Some(repo) = t.repos.get(t.repos_cursor).map(RepoRow::from) else {
                    return;
                };
                if !self.dest_confirmed {
                    self.open_dest_picker(None, Some(repo));
                    return;
                }
                self.download_repo(&repo);
            }
            Screen::Tree => {
                if self.transfer_busy() {
                    self.status = "transfer in flight: one at a time".into();
                    return;
                }
                let Some(row) = t.selected_row() else {
                    return;
                };
                let path = t.path.clone();
                match row.kind {
                    EntryKind::File => {
                        self.download_file(path, &row, self.anchor_base(), self.local.is_some());
                    }
                    EntryKind::Dir => {
                        let rel = self.full_rel(&path, &row.rel);
                        let Some(repo) = t.repo.clone() else {
                            return;
                        };
                        // From-root names mirror the repository: {dst}/{rel}/...
                        let dst = self.anchor_base();
                        self.begin_walk(RepoRow::from(&repo), rel, dst, false);
                    }
                }
            }
        }
    }

    /// The `D` verb: the download-as dialog. On Repos the whole repository,
    /// on Tree the current catalog below the breadcrumb.
    fn open_save_as(&mut self) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        let (repo, rel, name) = match t.screen {
            Screen::Repos => {
                let Some(repo) = t.repos.get(t.repos_cursor).map(RepoRow::from) else {
                    return;
                };
                let name = repo.name.clone();
                (repo, String::new(), name)
            }
            Screen::Tree => {
                let Some(repo) = t.repo.clone() else {
                    return;
                };
                let rel = t.path.join("/");
                let name = t.path.last().cloned().unwrap_or_else(|| repo.name.clone());
                (RepoRow::from(&repo), rel, name)
            }
        };
        self.mode = Mode::SaveAs(SaveAsDialog {
            dir: self.destination.display().to_string(),
            name,
            field: SaveAsField::Dir,
            scope: DlScope { repo, rel },
        });
    }

    /// The Enter of the download-as dialog: from the folder the cursor walks
    /// to the name, from the name the walk starts. Nothing is written before.
    fn confirm_save_as(&mut self, dialog: SaveAsDialog) {
        match dialog.field {
            SaveAsField::Dir => {
                let mut next = dialog;
                next.field = SaveAsField::Name;
                self.mode = Mode::SaveAs(next);
            }
            SaveAsField::Name => {
                let name = dialog.name.trim().to_owned();
                if name.is_empty() || name == "." || name == ".." {
                    self.status = "the name is empty or a dot segment".into();
                    return;
                }
                if name.contains('/') || name.contains('\\') || name.contains('\0') {
                    self.status = "the name carries a slash or a nul byte".into();
                    return;
                }
                let raw_dir = dialog.dir.trim().to_owned();
                if raw_dir.is_empty() {
                    self.status = "the destination is empty".into();
                    return;
                }
                let dst = self.resolve_path(Path::new(&raw_dir));
                self.destination = dst.clone();
                self.dest_confirmed = true;
                self.mode = Mode::Normal;
                // Scope-relative names land the subtree straight under {dir}/{name}.
                self.begin_walk(dialog.scope.repo, dialog.scope.rel, dst.join(name), true);
            }
        }
    }

    /// The card's `s`: download the file into the session destination.
    fn card_download(&mut self, card: Card) {
        let Card::File(card) = card else {
            return;
        };
        // `card.rel` is the path from the repository root, and `full_rel`
        // appends the row name itself: the row carries only the name, so a
        // nested file composes `{parents}/{name}` exactly once.
        let mut path: Vec<String> = card
            .rel
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        path.pop();
        let row = Row {
            name: card.name.clone(),
            rel: card.name.clone(),
            kind: EntryKind::File,
            depth: 0,
            expanded: false,
            loading: false,
        };
        self.mode = Mode::Card(Card::File(card));
        self.download_file(path, &row, self.destination.clone(), false);
    }

    /// The whole repository of one row into `{anchor}/{repo.name}`.
    fn download_repo(&mut self, repo: &RepoRow) {
        let dst = self.anchor_base().join(&repo.name);
        self.begin_walk(repo.clone(), String::new(), dst, false);
    }

    /// One file with the direct get primitive, digest-checked against the
    /// `.sha256` sibling when present. `flat` lands the file straight under
    /// `base` as `{base}/{name}` (the local pane anchor); otherwise the path
    /// from the repository root mirrors below `base`.
    fn download_file(&mut self, path: Vec<String>, row: &Row, base: PathBuf, flat: bool) {
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if self.transfer_busy() {
            self.status = "transfer in flight: one at a time".into();
            return;
        }
        let Some(repo) = t.repo.clone() else {
            return;
        };
        let rel = self.full_rel(&path, &row.rel);
        let url = format!("{}{}", t.here_url(), row.rel);
        let out = if flat {
            base.join(&row.name)
        } else {
            base.join(&rel)
        };
        let repo_row = RepoRow::from(&repo);
        self.transfer = Some(Transfer {
            repo_url: repo_row.dir_url(),
            server: Some(host_of(&repo_row.url)),
            repo_name: Some(repo_row.name.clone()),
            dst: base.clone(),
            files: Some(1),
            retry: Some(Retry::Get {
                url: url.clone(),
                out: out.clone(),
                dst: base.clone(),
                repo: repo_row,
            }),
            ..Transfer::default()
        });
        self.status = format!("downloading {}", row.name);
        let handle = net::fetch_one(self.tx.clone(), url, out, base, self.auth.clone());
        if let Some(dl) = self.transfer.as_mut() {
            dl.handle = Some(handle);
        }
    }

    /// The path from the repository root: the descended folders plus the row.
    fn full_rel(&self, path: &[String], row_rel: &str) -> String {
        if path.is_empty() {
            row_rel.to_owned()
        } else {
            format!("{}/{}", path.join("/"), row_rel)
        }
    }

    /// Starts a subtree walk. `scope_relative` strips the scope prefix from
    /// the collected names, so they land straight under `dst` (the dialog's
    /// `{dir}/{name}`); from-root names mirror the repository under `dst`.
    fn begin_walk(&mut self, repo: RepoRow, rel: String, dst: PathBuf, scope_relative: bool) {
        if self.transfer_busy() {
            self.status = "transfer in flight: one at a time".into();
            return;
        }
        let Some(t) = self.tabs.get(self.tab) else {
            return;
        };
        if t.loading {
            self.status = "a listing is in flight".into();
            return;
        }
        let scope_label = if rel.is_empty() {
            format!("{}/", repo.name)
        } else {
            format!("{rel}/")
        };
        self.transfer = Some(Transfer {
            repo_url: repo.dir_url(),
            server: Some(host_of(&repo.url)),
            repo_name: Some(repo.name.clone()),
            dst: dst.clone(),
            files: None,
            scope: rel.clone(),
            scope_relative,
            retry: Some(Retry::Walk {
                repo: repo.clone(),
                rel: rel.clone(),
                dst: dst.clone(),
                scope_relative,
            }),
            ..Transfer::default()
        });
        let gen = {
            let t = self.tabs.get_mut(self.tab);
            match t {
                Some(t) => {
                    t.gen += 1;
                    t.gen
                }
                None => return,
            }
        };
        self.pending += 1;
        self.status = format!("walking {scope_label}");
        net::walk_for_download(
            self.tx.clone(),
            self.tab,
            gen,
            repo.dir_url(),
            rel,
            EntryKind::Dir,
            self.auth.clone(),
        );
    }

    // ---- message application --------------------------------------------

    /// Marks a flying connect as detached: its result opens no tab and
    /// writes no config preset.
    fn detach_connect(&mut self) {
        if self.connecting > 0 {
            self.connect_open = false;
        }
    }

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
        let attached = !connect || self.connect_open;
        match res {
            Ok(repos) if slot < self.tabs.len() => {
                // A refresh of an existing tab. The cursor returns to the row
                // it sat on, by name.
                let keep = self.tabs[slot].keep_cursor_name.take();
                let t = &mut self.tabs[slot];
                let count = repos.len();
                t.repos = repos;
                t.loading = false;
                let quiet = std::mem::take(&mut t.refresh_quiet);
                t.repos_cursor = keep
                    .and_then(|name| t.repos.iter().position(|r| r.name == name))
                    .unwrap_or(0);
                // The quiet refresh of a finished put leaves the summary up.
                if self.tab == slot && !quiet {
                    self.status = format!("{count} repositories on {}", t.server.name);
                }
            }
            Ok(_repos) if !attached => {
                // The form was cancelled while the connect flew: the server
                // joins the overlay list only, and the config stays untouched.
                if !self.servers.iter().any(|s| s.url == server.url) {
                    self.servers.push(server.clone());
                }
                self.status = format!("server {} connected, press s to open", server.url);
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
                    self.tabs[slot].refresh_quiet = false;
                }
                let retry = (e.exit_code() != 2).then(|| {
                    if connect {
                        Retry::Connect {
                            server: server.clone(),
                        }
                    } else {
                        Retry::Listing { tab: slot }
                    }
                });
                let ctx = ErrCtx {
                    server: Some(if connect {
                        server.url.clone()
                    } else {
                        server.name.clone()
                    }),
                    repository: None,
                    destination: None,
                    source: None,
                };
                let kind = if connect {
                    ErrKind::Connect
                } else {
                    ErrKind::Listing
                };
                self.raise_error(kind, &e, ctx, retry);
            }
        }
    }

    fn on_entries(&mut self, tab: usize, gen: u64, res: Result<Vec<Entry>, Error>) {
        let Some(t) = self.tabs.get_mut(tab) else {
            return;
        };
        t.loading = false;
        let quiet = std::mem::take(&mut t.refresh_quiet);
        if gen != t.gen {
            return;
        }
        match res {
            Ok(entries) => {
                let empty = entries.is_empty();
                t.entries = entries;
                t.screen = Screen::Tree;
                // A refresh keeps the filter and puts the cursor back on its
                // row; a descend or an open starts clean.
                match t.keep_cursor_name.take() {
                    Some(name) => {
                        let rows = t.rows();
                        t.tree_cursor = t
                            .row_idx()
                            .into_iter()
                            .position(|i| rows.get(i).is_some_and(|r| r.name == name))
                            .unwrap_or_else(|| {
                                t.tree_cursor.min(t.row_idx().len().saturating_sub(1))
                            });
                    }
                    None => {
                        t.tree_cursor = 0;
                        t.filter = None;
                        t.filter_edit = false;
                    }
                }
                // The quiet refresh of a finished put leaves the summary up.
                if self.tab == tab && !quiet {
                    self.status = if empty {
                        "the folder is empty, esc goes back up".into()
                    } else {
                        hint_tree()
                    };
                }
            }
            Err(e) => {
                let ctx = ErrCtx::of(self, tab);
                let retry = (e.exit_code() != 2).then_some(Retry::Listing { tab });
                self.raise_error(ErrKind::Listing, &e, ctx, retry);
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
                let ctx = ErrCtx::of(self, tab);
                let retry = (e.exit_code() != 2).then_some(Retry::Listing { tab });
                self.raise_error(ErrKind::Listing, &e, ctx, retry);
            }
        }
    }

    fn on_walk(&mut self, tab: usize, gen: u64, res: Result<Vec<ArtifactName>, Error>) {
        let fresh = self.tabs.get(tab).is_some_and(|t| gen == t.gen);
        // The walk belongs to the panel only while the panel waits for it:
        // a cancelled run, a settled one or an upload never listen to it.
        let ours = match self.transfer.as_ref() {
            Some(dl) => {
                dl.dir == Dir::Down && dl.files.is_none() && dl.is_running() && !dl.cancelled
            }
            None => false,
        };
        if !fresh {
            if ours {
                // The tab navigated away while the walk ran: the request is
                // void, and the panel it prepared must not stay running.
                self.transfer = None;
            }
            return;
        }
        match res {
            Ok(names) if names.is_empty() => {
                if !ours {
                    return;
                }
                if let Some(dl) = self.transfer.as_mut() {
                    dl.outcome = Some(DlOutcome::Failed("the subtree holds no files".into(), None));
                }
                self.status = "the subtree holds no files".into();
            }
            Ok(names) if names.is_empty() => {
                if let Some(dl) = self.transfer.as_mut() {
                    dl.outcome = Some(DlOutcome::Failed("the subtree holds no files".into(), None));
                }
                self.status = "the subtree holds no files".into();
            }
            Ok(mut names) => {
                if !ours {
                    return;
                }
                let Some(dl) = self.transfer.as_ref() else {
                    return;
                };
                let mut base = dl.repo_url.clone();
                if dl.scope_relative {
                    let prefix = format!("{}/", dl.scope);
                    let mut stripped = Vec::with_capacity(names.len());
                    for name in names.drain(..) {
                        let Some(rest) = name.as_str().strip_prefix(&prefix) else {
                            let e = Error::UnsafeName {
                                name: name.as_str().to_owned(),
                                reason: "does not sit below the walked scope".into(),
                            };
                            let ctx = ErrCtx::of(self, tab);
                            if let Some(dl) = self.transfer.as_mut() {
                                dl.outcome = Some(DlOutcome::Failed(e.to_string(), e.hint()));
                                dl.retry = None;
                            }
                            self.raise_error(ErrKind::Download, &e, ctx, None);
                            return;
                        };
                        match ArtifactName::parse(rest) {
                            Ok(parsed) => stripped.push(parsed),
                            Err(e) => {
                                let ctx = ErrCtx::of(self, tab);
                                if let Some(dl) = self.transfer.as_mut() {
                                    dl.outcome = Some(DlOutcome::Failed(e.to_string(), e.hint()));
                                    dl.retry = None;
                                }
                                self.raise_error(ErrKind::Download, &e, ctx, None);
                                return;
                            }
                        }
                    }
                    names = stripped;
                    base = net::dir_url(&format!("{}{}", dl.repo_url, dl.scope));
                }
                let count = names.len();
                let unit = if count == 1 { "file" } else { "files" };
                let dst = dl.dst.clone();
                if let Some(dl) = self.transfer.as_mut() {
                    dl.retry = Some(Retry::Transfer {
                        repo_url: base.clone(),
                        names: names.clone(),
                        dst: dst.clone(),
                    });
                }
                self.status = format!("plan: {count} {unit} -> {}", dst.display());
                let handle =
                    net::start_download(self.tx.clone(), base, names, dst, self.auth.clone());
                if let Some(dl) = self.transfer.as_mut() {
                    dl.handle = Some(handle);
                }
            }
            Err(e) => {
                if !ours {
                    return;
                }
                let ctx = ErrCtx::of(self, tab);
                let retry = self
                    .transfer
                    .as_ref()
                    .and_then(|dl| (e.exit_code() != 2).then(|| dl.retry.clone()).flatten());
                if let Some(dl) = self.transfer.as_mut() {
                    dl.outcome = Some(DlOutcome::Failed(e.to_string(), e.hint()));
                }
                self.raise_error(ErrKind::Download, &e, ctx, retry);
            }
        }
    }

    /// The HEAD answer lands on its card only when the card is still open on
    /// the same tab, the same generation and the same path: the row token.
    fn on_head(&mut self, tab: usize, gen: u64, rel: String, res: Result<HeadInfo, Error>) {
        let Mode::Card(Card::File(mut card)) = self.mode.clone() else {
            return;
        };
        if card.tab != tab || card.gen != gen || card.rel != rel {
            return;
        }
        card.size = match res {
            Ok(info) => match info.size {
                Some(size) => SizeState::Known(size),
                None => SizeState::Unknown,
            },
            Err(e) => SizeState::Failed(e.to_string()),
        };
        self.mode = Mode::Card(Card::File(card));
    }

    /// The `.sha256` sibling answer: the same row token rule as the HEAD.
    fn on_sibling(
        &mut self,
        tab: usize,
        gen: u64,
        rel: String,
        res: Result<Option<String>, Error>,
    ) {
        let Mode::Card(Card::File(mut card)) = self.mode.clone() else {
            return;
        };
        if card.tab != tab || card.gen != gen || card.rel != rel {
            return;
        }
        card.sha = match res {
            Ok(None) => ShaState::Absent,
            Ok(Some(hex)) => ShaState::Hex(hex),
            Err(e) => ShaState::Failed(e.to_string()),
        };
        self.mode = Mode::Card(Card::File(card));
    }

    fn on_dl(&mut self, ev: DlEv) {
        let Some(dl) = self.transfer.as_mut() else {
            return;
        };
        if dl.cancelled {
            // The abort landed: the stragglers of the aborted task change nothing.
            return;
        }
        match ev {
            DlEv::Start { local, files } => {
                match dl.dir {
                    Dir::Down => dl.dst = local,
                    Dir::Up => dl.src = Some(local),
                }
                // An upload starts with an unknown count: the scan inside the
                // task answers, the plan event carries it in.
                if files.is_some() {
                    dl.files = files;
                }
            }
            DlEv::Plan { transfer, skip } => {
                dl.planned = Some((transfer, skip));
                // A download learns the count at Start; an upload learns it here.
                if dl.files.is_none() {
                    dl.files = Some(transfer);
                }
            }
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
                match dl.dir {
                    Dir::Up => {
                        let pos = self
                            .tabs
                            .get(self.tab)
                            .map(|t| t.breadcrumb())
                            .unwrap_or_default();
                        self.status = format!(
                            "uploaded {}, skipped {} -> {}",
                            summary.uploaded, summary.skipped, pos
                        );
                        // A finished put quietly refreshes the position it
                        // went into, when the tab still stands there.
                        let base = dl.repo_url.clone();
                        self.quiet_refresh_at(&base);
                    }
                    Dir::Down => {
                        self.status = format!(
                            "downloaded {}, skipped {}, failed {} -> {}",
                            summary.downloaded,
                            summary.skipped,
                            summary.failed.len(),
                            dl.dst.display()
                        );
                        // The pane re-reads the folder the download landed in.
                        let landed = dl.dst.clone();
                        if let Some(pane) = self.local.as_mut() {
                            if pane.cwd == landed {
                                pane.loading = true;
                                let cwd = pane.cwd.clone();
                                net::local_entries(self.tx.clone(), cwd);
                            }
                        }
                    }
                }
            }
            DlEv::Done(Err(e)) => {
                let hint = e.hint();
                dl.outcome = Some(DlOutcome::Failed(e.to_string(), hint.clone()));
                let retry = dl.retry.clone().filter(|_| e.exit_code() != 2);
                let ctx = ErrCtx {
                    server: dl.server.clone(),
                    repository: dl.repo_name.clone(),
                    destination: (dl.dir == Dir::Down).then(|| dl.dst.clone()),
                    source: dl.src.clone(),
                };
                let kind = match dl.dir {
                    Dir::Up => ErrKind::Upload,
                    Dir::Down => ErrKind::Download,
                };
                self.raise_error(kind, &e, ctx, retry);
            }
        }
    }

    /// Reruns the operation behind the `r` of an error modal.
    fn run_retry(&mut self, retry: Retry) {
        match retry {
            Retry::Listing { tab } => self.refresh_tab(tab),
            Retry::Connect { server } => {
                // The flag belongs to `connect_server`: a retry while another
                // connect is still flying must not attach that old result.
                self.connect_server(server);
            }
            Retry::Walk {
                repo,
                rel,
                dst,
                scope_relative,
            } => self.begin_walk(repo, rel, dst, scope_relative),
            Retry::Get {
                url,
                out,
                dst,
                repo,
            } => {
                if self.transfer_busy() {
                    self.status = "transfer in flight: one at a time".into();
                    return;
                }
                self.transfer = Some(Transfer {
                    repo_url: repo.dir_url(),
                    server: Some(host_of(&repo.url)),
                    repo_name: Some(repo.name.clone()),
                    dst: dst.clone(),
                    files: Some(1),
                    retry: Some(Retry::Get {
                        url: url.clone(),
                        out: out.clone(),
                        dst: dst.clone(),
                        repo,
                    }),
                    ..Transfer::default()
                });
                self.status = format!("downloading {}", url.rsplit('/').next().unwrap_or(&url));
                let handle = net::fetch_one(self.tx.clone(), url, out, dst, self.auth.clone());
                if let Some(dl) = self.transfer.as_mut() {
                    dl.handle = Some(handle);
                }
            }
            Retry::Transfer {
                repo_url,
                names,
                dst,
            } => {
                if self.transfer_busy() {
                    self.status = "transfer in flight: one at a time".into();
                    return;
                }
                let count = names.len();
                self.status = format!("plan: {count} files -> {}", dst.display());
                let handle =
                    net::start_download(self.tx.clone(), repo_url, names, dst, self.auth.clone());
                if let Some(dl) = self.transfer.as_mut() {
                    dl.handle = Some(handle);
                }
            }
            Retry::Put { src, base } => self.start_put(base, src),
        }
    }
}

/// The operation that failed: the modal headline, refined by the error class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErrKind {
    /// A walk, a get or a transfer refused.
    Download,
    /// A scan, a marker run or an upload refused.
    Upload,
    /// A listing refused.
    Listing,
    /// A connect refused.
    Connect,
}

impl ErrKind {
    /// The modal headline. Access refusals name themselves.
    fn title(self, e: &Error) -> &'static str {
        if matches!(e, Error::Auth { .. }) {
            return "access denied";
        }
        match self {
            ErrKind::Download => "download failed",
            ErrKind::Upload => "upload failed",
            ErrKind::Listing => "listing failed",
            ErrKind::Connect => "connect failed",
        }
    }
}

/// The non-empty facts an error modal can attach to the error itself.
#[derive(Debug, Clone, Default)]
struct ErrCtx {
    /// The server label or URL.
    server: Option<String>,
    /// The repository name.
    repository: Option<String>,
    /// The destination folder.
    destination: Option<PathBuf>,
    /// The source folder of an upload.
    source: Option<PathBuf>,
}

impl ErrCtx {
    /// The context of a tab: its server and, inside a tree, its repository.
    fn of(app: &App, tab: usize) -> Self {
        let Some(t) = app.tabs.get(tab) else {
            return Self::default();
        };
        Self {
            server: Some(t.server.name.clone()),
            repository: t.repo.as_ref().map(|r| r.name.clone()),
            destination: None,
            source: None,
        }
    }
}

/// The human cause of an error: one sentence, no colon chains, server-fed
/// names escaped. The hint stays with the core, verbatim.
#[must_use]
pub fn cause_of(e: &Error) -> String {
    match e {
        Error::Mismatch { name, .. } => format!(
            "the downloaded copy of '{}' does not match the digest the server pins",
            name.escape_debug()
        ),
        Error::Incomplete { names } => {
            let list = names
                .iter()
                .map(|n| n.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("these names did not finish: {list}")
        }
        Error::UnsafeName { name, .. } => format!(
            "the name '{}' is not a legal artifact path",
            name.escape_debug()
        ),
        Error::Missing { names } => {
            let list = names
                .iter()
                .map(|n| n.escape_debug().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("nothing to download, '{list}' is on neither side")
        }
        Error::Enumerate { url, .. } => format!(
            "the server cannot list the contents of '{}'",
            url.escape_debug()
        ),
        Error::Auth { .. } => "the server refused the credentials or none were given".to_owned(),
        Error::Transport { url, .. } => format!(
            "the connection to '{}' failed on the way",
            url.escape_debug()
        ),
        Error::Http { status, url } => format!(
            "the server answered HTTP {status} for '{}'",
            url.escape_debug()
        ),
        Error::ServiceMissing { url, .. } => format!(
            "the server at '{}' has no Nexus service API",
            url.escape_debug()
        ),
        Error::ReadOnly { url, status } => format!(
            "the repository '{}' answered HTTP {status} to a deletion, it is read-only",
            url.escape_debug()
        ),
        Error::Misuse(_) => "the request is malformed and cannot be retried".to_owned(),
        Error::Io { path, .. } => format!(
            "the local filesystem refused a step on '{}'",
            path.escape_debug()
        ),
        // The core enum is non-exhaustive: new variants surface raw until mapped.
        _ => format!("the operation failed: {e}"),
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
    "enter: open · d: download · D: as · i: card · o: destination · r: refresh · ?: help · q: quit"
        .into()
}

/// The idle status hint of the tree screen.
#[must_use]
pub fn hint_tree() -> String {
    "enter: inspect · d: download · D: as · o: destination · /: filter · r: refresh · ?: help · q: quit"
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

    /// A real terminal delivers capitals as the uppercase char plus Shift.
    fn key_shifted(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
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

    /// Presses the `r` of the open error modal.
    fn modal_retry_runs(app: &mut App) {
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("no error modal is open");
        };
        assert!(modal.retry.is_some(), "the modal carries no retry");
        // The real key handler closes the modal before the retry runs.
        app.handle(Msg::Key(key(KeyCode::Char('r'))));
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
        // Enter applies and closes the input: the tree is browsable again.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.tabs[0].filter.is_some());
        assert!(!app.tabs[0].filter_edit);
        // Esc while the input is open clears the filter completely.
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        assert!(app.tabs[0].filter_edit, "the input reopens for editing");
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
    async fn enter_on_a_file_opens_the_card_instead_of_downloading() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        // The card is up, HEAD and the sha sibling fly, nothing downloads.
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("enter must open the card");
        };
        assert_eq!(card.name, "README.txt");
        assert_eq!(card.rel, "README.txt");
        assert_eq!(
            card.url,
            "http://127.0.0.1:1/repository/raw-main/README.txt"
        );
        assert_eq!(app.pending, 2);
        assert!(
            app.transfer.is_none(),
            "the primary verb never starts a transfer"
        );
        assert!(app.needs_tick(), "the card waits for its metadata");
        // HEAD and the sibling land in arrival order: both name the row.
        for _ in 0..2 {
            match rx.recv().await.unwrap() {
                Msg::Head { tab, gen, rel, .. } => {
                    assert_eq!((tab, gen), (0, app.tabs[0].gen));
                    assert_eq!(rel, "README.txt");
                }
                Msg::Sibling { rel, .. } => assert_eq!(rel, "README.txt"),
                other => panic!("unexpected message: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn the_head_answer_is_guarded_by_the_row_token() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let gen = app.tabs[0].gen;
        // An answer of another row, another tab and another generation: dropped.
        app.handle(Msg::Head {
            tab: 0,
            gen,
            rel: "app".into(),
            res: Ok(HeadInfo {
                status: 200,
                size: Some(1),
                content_type: None,
            }),
        });
        app.handle(Msg::Head {
            tab: 1,
            gen,
            rel: "README.txt".into(),
            res: Ok(HeadInfo {
                status: 200,
                size: Some(2),
                content_type: None,
            }),
        });
        app.handle(Msg::Head {
            tab: 0,
            gen: gen + 1,
            rel: "README.txt".into(),
            res: Ok(HeadInfo {
                status: 200,
                size: Some(3),
                content_type: None,
            }),
        });
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("the card stays open");
        };
        assert_eq!(card.size, SizeState::Pending, "foreign answers never paint");
        // The token match lands.
        app.handle(Msg::Head {
            tab: 0,
            gen,
            rel: "README.txt".into(),
            res: Ok(HeadInfo {
                status: 200,
                size: Some(21),
                content_type: Some("text/plain".into()),
            }),
        });
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("the card stays open");
        };
        assert_eq!(card.size, SizeState::Known(21));
        assert!(!app.needs_tick() || card.sha == ShaState::Pending);
    }

    #[tokio::test]
    async fn the_sibling_answer_fills_the_sha_line_of_the_card() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let gen = app.tabs[0].gen;
        app.handle(Msg::Sibling {
            tab: 0,
            gen,
            rel: "README.txt".into(),
            res: Ok(Some("ab".repeat(32))),
        });
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("the card stays open");
        };
        assert_eq!(card.sha, ShaState::Hex("ab".repeat(32)));
        // No marker on the server: the card says so instead of guessing.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let gen = app.tabs[0].gen;
        app.handle(Msg::Sibling {
            tab: 0,
            gen,
            rel: "README.txt".into(),
            res: Ok(None),
        });
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("the card stays open");
        };
        assert_eq!(card.sha, ShaState::Absent);
    }

    #[tokio::test]
    async fn the_card_downloads_with_s_and_copies_with_c() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        // The card stays open, the transfer takes the single slot.
        assert!(matches!(app.mode, Mode::Card(_)));
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { local: dst, files }) => {
                assert_eq!(files, Some(1));
                assert_eq!(dst, PathBuf::from("dl"));
            }
            other => panic!("unexpected message: {other:?}"),
        }
        assert!(app.transfer_busy(), "the card download holds the slot");
        // A second `s` waits its turn.
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        assert_eq!(app.status, "transfer in flight: one at a time");
        // `c` copies the url: the copied toast is the confirmation.
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert_eq!(app.toasts.back().map(|t| t.text.as_str()), Some("copied"));
    }

    #[tokio::test]
    async fn the_card_o_changes_the_destination_and_returns() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Key(key(KeyCode::Char('o'))));
        let Mode::Dest(picker) = app.mode.clone() else {
            panic!("o opens the picker");
        };
        assert!(
            picker.back_to_card.is_some(),
            "the picker returns to the card"
        );
        assert_eq!(picker.buffer, "dl", "the picker prefills the destination");
        for c in "/out".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.destination.ends_with("out"), "{:?}", app.destination);
        assert!(app.dest_confirmed);
        assert!(
            matches!(app.mode, Mode::Card(_)),
            "the card gets its layer back"
        );
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
    async fn the_dialog_downloads_the_current_folder_as() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        // Descend into app/ and let the child listing land.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![entry("core", EntryKind::Dir)]),
        });
        // D opens the dialog: folder prefilled with the destination, name with
        // the folder. Nothing walks before Enter on the name.
        app.handle(Msg::Key(key(KeyCode::Char('D'))));
        let Mode::SaveAs(dialog) = app.mode.clone() else {
            panic!("D opens the dialog");
        };
        assert_eq!(dialog.dir, "dl");
        assert_eq!(dialog.name, "app");
        assert_eq!(dialog.scope.rel, "app");
        assert!(rx.try_recv().is_err(), "no walk before the confirmation");
        // Enter on the dir field walks to the name, Enter on the name starts.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(matches!(app.mode, Mode::SaveAs(_)));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.status, "walking app/");
        let dst = app.transfer.as_ref().map(|dl| dl.dst.clone()).unwrap();
        assert!(dst.ends_with("app"), "{dst:?}");
        // The walk runs over the scope, names stripped to it.
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
        // A slash in the name is refused before anything happens.
        app.handle(Msg::Key(key(KeyCode::Char('D'))));
        let Mode::SaveAs(dialog) = app.mode.clone() else {
            panic!("D opens the dialog");
        };
        app.handle(Msg::Key(key(KeyCode::Tab)));
        for c in "bad/name".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        let before = app.pending;
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.status.contains("slash"), "{:?}", app.status);
        assert_eq!(app.pending, before, "nothing spawned");
        assert!(matches!(app.mode, Mode::SaveAs(_)));
        let _ = dialog;
        // Esc cancels: nothing is created, no walk is flying.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[tokio::test]
    async fn the_dialog_renames_the_whole_repository_from_repos() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        // d on Repos with the untouched default destination: the picker first.
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let Mode::Dest(picker) = app.mode.clone() else {
            panic!("the unconfirmed destination asks first");
        };
        assert_eq!(
            picker.pending_repo.as_ref().map(|r| r.name.as_str()),
            Some("raw-main")
        );
        assert_eq!(picker.buffer, "dl");
        // Confirm: the destination is set and the walk starts.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.destination.ends_with("dl"), "{:?}", app.destination);
        assert!(app.dest_confirmed);
        assert_eq!(app.status, "walking raw-main/");
        // Once confirmed, the next d goes straight to the walk.
        app.transfer = None;
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert_eq!(app.status, "walking raw-main/");
        let dst = app.transfer.as_ref().map(|dl| dl.dst.clone()).unwrap();
        assert!(dst.ends_with("raw-main"), "{dst:?}");
    }

    #[tokio::test]
    async fn uppercase_paths_and_shift_d_survive_real_terminal_modifiers() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        // An uppercase path character arrives as Char with Shift.
        app.handle(Msg::Key(key(KeyCode::Char('o'))));
        app.handle(Msg::Key(key(KeyCode::Backspace)));
        app.handle(Msg::Key(key(KeyCode::Backspace)));
        for ch in "/tmp/Outs".chars() {
            let code = KeyCode::Char(ch);
            app.handle(Msg::Key(if ch.is_uppercase() {
                key_shifted(code)
            } else {
                key(code)
            }));
        }
        assert!(
            matches!(&app.mode, Mode::Dest(p) if p.buffer == "/tmp/Outs"),
            "{:?}",
            app.mode
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.destination.ends_with("Outs"), "{:?}", app.destination);
        // Shift+D opens the download-as dialog on the Tree screen.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        app.handle(Msg::Key(key_shifted(KeyCode::Char('D'))));
        assert!(matches!(app.mode, Mode::SaveAs(_)), "{:?}", app.mode);
        assert!(
            rx.try_recv().is_err(),
            "the dialog and picker never touch the network"
        );
    }

    #[tokio::test]
    async fn the_destination_picker_confirms_and_cancels() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('o'))));
        assert_eq!(
            app.mode,
            Mode::Dest(DestPicker {
                buffer: "dl".into(),
                back_to_card: None,
                pending_repo: None,
            })
        );
        for c in "/tmp/outs".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        app.handle(Msg::Key(key(KeyCode::Enter)));
        // A relative buffer resolves against the launch directory.
        assert!(app.destination.ends_with("outs"), "{:?}", app.destination);
        assert!(app.destination.is_absolute());
        assert!(app.status.starts_with("destination: "));
        // Esc cancels: the destination stays.
        app.handle(Msg::Key(key(KeyCode::Char('o'))));
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.destination.ends_with("outs"), "{:?}", app.destination);
        assert!(
            rx.try_recv().is_err(),
            "the picker never touches the network"
        );
    }

    /// Types the given text into the open source picker and confirms it.
    fn confirm_source_with(app: &mut App, path: &Path) {
        let Mode::PickSource(picker) = app.mode.clone() else {
            panic!("the source picker is open");
        };
        for _ in 0..picker.buffer.chars().count() {
            app.handle(Msg::Key(key(KeyCode::Backspace)));
        }
        for c in path.display().to_string().chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        app.handle(Msg::Key(key(KeyCode::Enter)));
    }

    /// A real local folder with one file inside: the put source of the tests.
    fn put_source() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), b"fn main() {}\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn the_source_picker_prefills_and_confirms_a_real_folder() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.base_dir = PathBuf::from("/tmp/put-seed");
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        let Mode::PickSource(picker) = app.mode.clone() else {
            panic!("p opens the source picker");
        };
        assert_eq!(
            picker.buffer, "/tmp/put-seed",
            "the picker prefills the launch directory"
        );
        let dir = put_source();
        confirm_source_with(&mut app, dir.path());
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.upload_source.as_deref(), Some(dir.path()));
        assert_eq!(
            app.status,
            format!("put {} -> raw-main", dir.path().display())
        );
        let dl = app.transfer.as_ref().expect("the put takes the slot");
        assert_eq!(dl.dir, Dir::Up);
        assert_eq!(dl.src.as_deref(), Some(dir.path()));
        assert_eq!(dl.repo_url, "http://127.0.0.1:1/repository/raw-main/");
        assert!(
            matches!(dl.retry, Some(Retry::Put { .. })),
            "a failed put runs again"
        );
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { local, files }) => {
                assert_eq!(local, dir.path());
                assert_eq!(files, None, "the scan inside the task knows the count");
            }
            other => panic!("unexpected message: {other:?}"),
        }
        // The next put prefills with the confirmed source.
        app.transfer = None;
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        let Mode::PickSource(picker) = app.mode.clone() else {
            panic!("p opens the source picker again");
        };
        assert_eq!(
            picker.buffer,
            dir.path().display().to_string(),
            "the picker prefills the last confirmed source"
        );
    }

    #[tokio::test]
    async fn the_source_picker_refuses_a_file_and_esc_cancels() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        let file = tempfile::tempdir().unwrap();
        let path = file.path().join("plain.txt");
        std::fs::write(&path, b"x").unwrap();
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        confirm_source_with(&mut app, &path);
        assert!(matches!(app.mode, Mode::PickSource(_)), "the picker stays");
        assert_eq!(app.status, format!("not a directory: {}", path.display()));
        assert!(app.transfer.is_none(), "nothing was prepared");
        // Esc cancels: nothing spawned, nothing remembered.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.upload_source.is_none());
        assert!(
            rx.try_recv().is_err(),
            "the picker never touches the network"
        );
    }

    #[tokio::test]
    async fn put_on_the_repositories_screen_is_refused() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        assert_eq!(app.status, "the put works in the tree");
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.transfer.is_none());
    }

    #[tokio::test]
    async fn a_put_holds_the_single_transfer_slot() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        let dir = put_source();
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        confirm_source_with(&mut app, dir.path());
        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, Msg::Dl(DlEv::Start { .. })), "{msg:?}");
        assert!(app.transfer_busy(), "the put holds the slot");
        // A second transfer politely waits its turn, from both verbs.
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert_eq!(app.status, "transfer in flight: one at a time");
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        assert_eq!(app.status, "transfer in flight: one at a time");
        assert_eq!(app.mode, Mode::Normal, "the gate never opens the picker");
        assert!(rx.try_recv().is_err(), "no second transfer was spawned");
    }

    /// A prepared upload the fold tests can feed events into: no network.
    fn upload_in_flight(app: &mut App) {
        let dir = put_source();
        app.transfer = Some(Transfer {
            dir: Dir::Up,
            repo_url: "http://127.0.0.1:1/repository/raw-main/".into(),
            server: Some("127.0.0.1:1".into()),
            repo_name: Some("raw-main".into()),
            src: Some(dir.path().to_path_buf()),
            retry: Some(Retry::Put {
                src: dir.path().to_path_buf(),
                base: "http://127.0.0.1:1/repository/raw-main/".into(),
            }),
            ..Transfer::default()
        });
    }

    #[tokio::test]
    async fn an_upload_folds_the_plan_bytes_and_a_summary() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        app.handle(Msg::Dl(DlEv::Plan {
            transfer: 3,
            skip: 1,
        }));
        app.handle(Msg::Dl(DlEv::Bytes {
            name: "a.rs".into(),
            done: 40,
            total: Some(100),
        }));
        {
            let dl = app.transfer.as_ref().unwrap();
            assert_eq!(dl.planned, Some((3, 1)));
            assert_eq!(dl.files, Some(3), "the plan event carries the count");
            assert_eq!(dl.active.get("a.rs"), Some(&(40, Some(100))));
        }
        app.handle(Msg::Dl(DlEv::Retry {
            name: "a.rs".into(),
            attempt: 1,
            reason: "reset by peer".into(),
        }));
        assert_eq!(
            app.transfer.as_ref().unwrap().note.as_deref(),
            Some("a.rs: retry 1: reset by peer")
        );
        app.handle(Msg::Dl(DlEv::FileDone {
            name: "a.rs".into(),
            skipped: false,
        }));
        app.handle(Msg::Dl(DlEv::FileDone {
            name: "b.rs".into(),
            skipped: true,
        }));
        {
            let dl = app.transfer.as_ref().unwrap();
            assert_eq!(dl.finished, 1);
            assert_eq!(dl.skipped, 1);
            assert!(dl.active.is_empty());
        }
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            uploaded: 3,
            downloaded: 0,
            skipped: 1,
            removed: 0,
            failed: Vec::new(),
        }))));
        assert_eq!(app.status, "uploaded 3, skipped 1 -> raw-main");
        let dl = app.transfer.as_ref().unwrap();
        assert!(matches!(dl.outcome, Some(DlOutcome::Done(_))));
        assert!(!app.transfer_busy(), "the slot is free again");
        // The position the put went into quietly refreshes.
        assert!(
            app.tabs[0].loading,
            "the finished put refreshed the position"
        );
        match rx.recv().await.unwrap() {
            Msg::Entries { tab, gen, .. } => {
                assert_eq!(tab, 0);
                assert_eq!(gen, app.tabs[0].gen, "the refresh rides the new gen");
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_failed_upload_opens_the_upload_modal_with_a_retry() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        let src = app.transfer.as_ref().unwrap().src.clone().unwrap();
        let e = Error::Transport {
            url: "http://127.0.0.1:1/repository/raw-main/".into(),
            detail: "connection refused".into(),
        };
        app.handle(Msg::Dl(DlEv::Done(Err(e))));
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the upload refusal opens the error modal");
        };
        assert_eq!(modal.title, "upload failed");
        assert!(
            modal
                .facts
                .contains(&("source".into(), src.display().to_string())),
            "{:?}",
            modal.facts
        );
        assert!(matches!(modal.retry, Some(Retry::Put { .. })));
        assert!(!app.transfer_busy(), "the slot is free again");
        // r reruns the same put into the same base.
        modal_retry_runs(&mut app);
        assert_eq!(app.status, format!("put {} -> raw-main", src.display()));
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { local, .. }) => assert_eq!(local, src),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_empty_source_opens_the_upload_modal_without_retry() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        let src = app.transfer.as_ref().unwrap().src.clone().unwrap();
        let e = Error::Misuse(format!(
            "nothing to upload: {} holds no artifacts",
            src.display()
        ));
        app.handle(Msg::Dl(DlEv::Done(Err(e))));
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the empty scan opens the error modal");
        };
        assert_eq!(modal.title, "upload failed");
        assert!(modal.cause.contains("malformed"), "{:?}", modal.cause);
        assert!(modal.retry.is_none(), "a misuse cannot be retried");
        assert!(!app.transfer_busy());
    }

    #[tokio::test]
    async fn x_cancels_the_run_and_ignores_the_stragglers() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        // A live handle rides along, so the abort lands on a real task.
        app.transfer.as_mut().unwrap().handle = Some(tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }));
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "cancelled");
        let dl = app.transfer.as_ref().unwrap();
        assert!(dl.cancelled);
        assert!(matches!(dl.outcome, Some(DlOutcome::Cancelled)));
        assert!(dl.handle.is_none(), "the aborted handle is dropped");
        assert!(!app.transfer_busy(), "the slot is free again");
        assert!(matches!(app.mode, Mode::Normal), "no modal on a cancel");
        // The stragglers of the aborted task change nothing.
        app.handle(Msg::Dl(DlEv::Bytes {
            name: "a.rs".into(),
            done: 10,
            total: None,
        }));
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            uploaded: 9,
            downloaded: 0,
            skipped: 0,
            removed: 0,
            failed: Vec::new(),
        }))));
        let dl = app.transfer.as_ref().unwrap();
        assert!(matches!(dl.outcome, Some(DlOutcome::Cancelled)));
        assert!(dl.active.is_empty(), "the straggler bytes were ignored");
        assert_eq!(app.status, "cancelled", "no fake summary rewrites the end");
        // x over a settled panel only reports.
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "nothing is running");
    }

    #[tokio::test]
    async fn x_during_a_walk_never_starts_its_download() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert!(app.transfer_busy(), "the walk holds the slot");
        // x cancels while the walk still flies: the slot frees at once.
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "cancelled");
        assert!(!app.transfer_busy());
        // The walk lands late: nobody waits for it, no download may start.
        app.handle(Msg::Walk {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![ArtifactName::parse("app/lib.rs").unwrap()]),
        });
        assert!(
            rx.try_recv().is_err(),
            "the cancelled walk never starts a download"
        );
        let dl = app.transfer.as_ref().unwrap();
        assert!(matches!(dl.outcome, Some(DlOutcome::Cancelled)));
        assert!(!app.transfer_busy(), "the slot stays clean");
        // A navigation away during the walk voids it without a zombie panel.
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert!(app.transfer_busy());
        app.handle(Msg::Key(key(KeyCode::Esc)));
        let gen = app.tabs[0].gen - 1;
        app.handle(Msg::Walk {
            tab: 0,
            gen,
            res: Ok(vec![ArtifactName::parse("app/lib.rs").unwrap()]),
        });
        assert!(rx.try_recv().is_err(), "no download for a stale walk");
        assert!(app.transfer.is_none(), "the stale walk released the slot");
    }

    #[tokio::test]
    async fn x_cancels_a_running_download_too() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.transfer = Some(Transfer {
            dir: Dir::Down,
            repo_url: "http://127.0.0.1:1/repository/raw-main/".into(),
            dst: PathBuf::from("dl"),
            handle: Some(tokio::spawn(async {
                loop {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            })),
            ..Transfer::default()
        });
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "cancelled");
        let dl = app.transfer.as_ref().unwrap();
        assert!(dl.cancelled);
        assert!(matches!(dl.outcome, Some(DlOutcome::Cancelled)));
        // The stragglers of the aborted download change nothing.
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            downloaded: 5,
            ..Summary::default()
        }))));
        assert!(matches!(
            app.transfer.as_ref().unwrap().outcome,
            Some(DlOutcome::Cancelled)
        ));
        assert_eq!(app.status, "cancelled");
    }

    #[tokio::test]
    async fn paste_fills_the_source_picker() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        assert!(matches!(app.mode, Mode::PickSource(_)));
        app.handle(Msg::Paste("/tmp/put-src".into()));
        let Mode::PickSource(picker) = app.mode.clone() else {
            panic!("the picker stays open");
        };
        assert!(
            picker.buffer.ends_with("/tmp/put-src"),
            "the paste lands in the buffer: {:?}",
            picker.buffer
        );
        assert!(rx.try_recv().is_err(), "paste never starts a transfer");
    }

    #[tokio::test]
    async fn the_quiet_refresh_skips_a_moved_away_position() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        // The user descended elsewhere while the put ran: the position moved.
        app.tabs[0].path = vec!["other".into()];
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            uploaded: 1,
            ..Summary::default()
        }))));
        assert_eq!(app.status, "uploaded 1, skipped 0 -> raw-main / other");
        assert!(!app.tabs[0].loading);
        assert!(
            rx.try_recv().is_err(),
            "a moved-away position never refreshes"
        );
        // A listing in flight at the moment of success is respected too.
        app.tabs[0].path.clear();
        app.tabs[0].loading = true;
        let gen = app.tabs[0].gen;
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            uploaded: 1,
            ..Summary::default()
        }))));
        assert_eq!(app.tabs[0].gen, gen, "no refresh under a flying listing");
        assert!(app.tabs[0].loading, "the flying listing stays in charge");
        let _ = rx;
    }

    #[tokio::test]
    async fn x_with_nothing_running_reports() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "nothing is running");
        assert!(app.transfer.is_none());
        // A finished transfer is not a running one.
        upload_in_flight(&mut app);
        app.transfer.as_mut().unwrap().outcome = Some(DlOutcome::Cancelled);
        app.handle(Msg::Key(key(KeyCode::Char('x'))));
        assert_eq!(app.status, "nothing is running");
    }

    #[tokio::test]
    async fn c_copies_the_url_of_the_selection() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert_eq!(app.toasts.back().map(|t| t.text.as_str()), Some("copied"));
        // On the tree the URL composes from the current position.
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert_eq!(app.toasts.back().map(|t| t.text.as_str()), Some("copied"));
        // An empty screen has nothing to copy and stays quiet.
        let (mut app, _rx) = app_with(vec![tab_with(vec![])], false);
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert!(app.toasts.is_empty());
    }

    #[tokio::test]
    async fn a_failed_transfer_keeps_the_add_server_form_intact() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        // The add form is open, the URL is half typed.
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        app.handle(Msg::Key(key(KeyCode::Char('a'))));
        for c in "http://10.0.0".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        // A background transfer fails over the form.
        let e = Error::Transport {
            url: "http://127.0.0.1:1/repository/raw-main/".into(),
            detail: "connection refused".into(),
        };
        app.handle(Msg::Dl(DlEv::Done(Err(e))));
        let Mode::Error { modal, back } = app.mode.clone() else {
            panic!("the failure opens the error modal");
        };
        assert_eq!(modal.title, "upload failed");
        assert!(
            matches!(back.as_deref(), Some(Mode::AddServer(url)) if url == "http://10.0.0"),
            "the modal remembers the form: {back:?}"
        );
        // Esc gives the layer back, text and all.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::AddServer("http://10.0.0".into()));
        // The form still works: the rest of the URL types on.
        for c in ".4:8081/".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        assert_eq!(app.mode, Mode::AddServer("http://10.0.0.4:8081/".into()));
    }

    #[tokio::test]
    async fn a_failed_transfer_keeps_the_destination_picker_intact() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        app.handle(Msg::Key(key(KeyCode::Char('o'))));
        let Mode::Dest(picker) = app.mode.clone() else {
            panic!("o opens the picker");
        };
        for _ in 0..picker.buffer.chars().count() {
            app.handle(Msg::Key(key(KeyCode::Backspace)));
        }
        for c in "/tmp/kept".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        let e = Error::Io {
            path: "/tmp/put-src".into(),
            detail: "permission denied".into(),
        };
        app.handle(Msg::Dl(DlEv::Done(Err(e))));
        let Mode::Error { modal, back } = app.mode.clone() else {
            panic!("the failure opens the error modal");
        };
        assert_eq!(modal.title, "upload failed");
        assert!(
            matches!(back.as_deref(), Some(Mode::Dest(_))),
            "the modal remembers the picker: {back:?}"
        );
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(matches!(app.mode, Mode::Dest(_)), "the picker is back");
        let Mode::Dest(picker) = app.mode.clone() else {
            panic!("the picker is back");
        };
        assert_eq!(picker.buffer, "/tmp/kept");
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[tokio::test]
    async fn a_dialog_survives_a_retry_behind_the_modal() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        upload_in_flight(&mut app);
        let src = app.transfer.as_ref().unwrap().src.clone().unwrap();
        // The download-as dialog is open with an edited name.
        app.handle(Msg::Key(key(KeyCode::Char('D'))));
        let Mode::SaveAs(dialog) = app.mode.clone() else {
            panic!("D opens the dialog");
        };
        app.handle(Msg::Key(key(KeyCode::Tab)));
        for _ in 0..dialog.name.chars().count() {
            app.handle(Msg::Key(key(KeyCode::Backspace)));
        }
        for c in "kept".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        // The upload fails behind the dialog.
        let e = Error::Transport {
            url: "http://127.0.0.1:1/repository/raw-main/".into(),
            detail: "connection refused".into(),
        };
        app.handle(Msg::Dl(DlEv::Done(Err(e))));
        assert!(matches!(app.mode, Mode::Error { .. }));
        // r reruns the put and hands the layer straight back.
        modal_retry_runs(&mut app);
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { local, .. }) => assert_eq!(local, src),
            other => panic!("unexpected message: {other:?}"),
        }
        let Mode::SaveAs(dialog) = app.mode.clone() else {
            panic!("the dialog comes back behind the retry");
        };
        assert_eq!(dialog.name, "kept");
        assert_eq!(dialog.field, SaveAsField::Name);
    }

    /// Opens the pane over a real folder: one folder, two files. Returns the
    /// tempdir (the listing token) and the answers to feed.
    fn open_pane(app: &mut App) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("sub")).unwrap();
        std::fs::write(root.path().join("a.txt"), b"aaa").unwrap();
        std::fs::write(root.path().join("b.bin"), b"bb").unwrap();
        let cwd = root.path().to_path_buf();
        app.base_dir = cwd.clone();
        app.term = Rect {
            width: 120,
            height: 40,
            x: 0,
            y: 0,
        };
        app.handle(Msg::Key(key(KeyCode::Char('v'))));
        (root, cwd)
    }

    fn pane_entries_answer(cwd: &Path) -> Msg {
        Msg::LocalEntries {
            cwd: cwd.to_path_buf(),
            res: Ok(vec![
                LocalEntry {
                    name: "b.bin".into(),
                    kind: EntryKind::File,
                    size: Some(2),
                },
                LocalEntry {
                    name: "a.txt".into(),
                    kind: EntryKind::File,
                    size: Some(3),
                },
                LocalEntry {
                    name: "sub".into(),
                    kind: EntryKind::Dir,
                    size: None,
                },
            ]),
        }
    }

    #[tokio::test]
    async fn v_opens_the_pane_and_sorts_the_listing() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        let (root, cwd) = open_pane(&mut app);
        assert_eq!(app.status, "dual-pane on");
        assert!(app.local_focus, "the pane takes the focus");
        let pane = app.local.as_ref().unwrap();
        assert!(pane.loading, "the listing is in flight");
        match rx.recv().await.unwrap() {
            Msg::LocalEntries { cwd: token, .. } => assert_eq!(token, cwd),
            other => panic!("unexpected message: {other:?}"),
        }
        app.handle(pane_entries_answer(&cwd));
        let pane = app.local.as_ref().unwrap();
        assert!(!pane.loading);
        // Folders first, names sorted below them.
        let names: Vec<&str> = pane.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["sub", "a.txt", "b.bin"]);
        assert_eq!(pane.cursor, 0);
        // tab switching never drops the pane: it is a session surface.
        app.handle(Msg::Key(key(KeyCode::Tab)));
        assert!(app.local.is_some());
        let _ = root;
    }

    #[tokio::test]
    async fn the_pane_navigates_and_refuses_the_remote_verbs() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        let (root, cwd) = open_pane(&mut app);
        app.handle(pane_entries_answer(&cwd));
        // The remote verbs refuse with the pointer to l.
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        assert_eq!(app.status, "switch to the remote pane (l)");
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        assert_eq!(app.status, "switch to the remote pane (l)");
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        assert_eq!(app.status, "the filter works in the tree");
        // Enter on a file reports, the local card does not exist.
        app.handle(Msg::Key(key(KeyCode::Char('j'))));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.status, "enter opens folders here");
        // Enter on a folder descends, esc climbs back up.
        app.handle(Msg::Key(key(KeyCode::Char('g'))));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        {
            let pane = app.local.as_ref().unwrap();
            assert_eq!(pane.cwd, cwd.join("sub"));
            assert!(pane.loading);
            assert!(pane.entries.is_empty());
        }
        app.handle(Msg::LocalEntries {
            cwd: cwd.join("sub"),
            res: Ok(vec![LocalEntry {
                name: "deep.txt".into(),
                kind: EntryKind::File,
                size: Some(1),
            }]),
        });
        app.handle(Msg::Key(key(KeyCode::Esc)));
        {
            let pane = app.local.as_ref().unwrap();
            assert_eq!(pane.cwd, cwd, "esc is one folder up");
            assert!(pane.loading);
        }
        // A stale answer of the abandoned folder is dropped.
        app.handle(Msg::LocalEntries {
            cwd: cwd.join("sub"),
            res: Ok(vec![]),
        });
        assert!(app.local.as_ref().unwrap().loading);
        // The filesystem root reports instead of failing.
        app.local.as_mut().unwrap().cwd = PathBuf::from("/");
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.status, "at the root of the filesystem");
        // l hands the focus to the remote side, h takes it back.
        app.handle(Msg::Key(key(KeyCode::Char('l'))));
        assert!(!app.local_focus);
        app.handle(Msg::Key(key(KeyCode::Char('h'))));
        assert!(app.local_focus);
        let _ = root;
    }

    #[tokio::test]
    async fn the_pane_gate_needs_eighty_columns() {
        let (mut app, _rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.term = Rect {
            width: 79,
            height: 40,
            x: 0,
            y: 0,
        };
        app.handle(Msg::Key(key(KeyCode::Char('v'))));
        assert_eq!(app.status, "the terminal is too narrow for dual-pane");
        assert!(app.local.is_none(), "the pane never opens narrow");
        app.term = Rect {
            width: 80,
            height: 40,
            x: 0,
            y: 0,
        };
        app.handle(Msg::Key(key(KeyCode::Char('v'))));
        assert!(app.local.is_some());
        // v closes the pane and the anchors return to the destination.
        app.handle(Msg::Key(key(KeyCode::Char('v'))));
        assert_eq!(app.status, "dual-pane off");
        assert!(app.local.is_none());
        assert!(!app.local_focus);
    }

    #[tokio::test]
    async fn the_pane_anchors_downloads_and_the_put_picker() {
        let (mut app, rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("README.txt", EntryKind::File),
            ])],
            false,
        );
        let (root, cwd) = open_pane(&mut app);
        app.handle(pane_entries_answer(&cwd));
        app.handle(Msg::Key(key(KeyCode::Char('l'))));
        app.dest_confirmed = true;
        // A file lands flat: {cwd}/{name}.
        app.handle(Msg::Key(key(KeyCode::End)));
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        {
            let dl = app.transfer.as_ref().unwrap();
            let Retry::Get { out, dst, .. } = dl.retry.as_ref().unwrap() else {
                panic!("the file download retries as a get");
            };
            assert_eq!(out, &cwd.join("README.txt"), "flat into the pane folder");
            assert_eq!(dst, &cwd);
        }
        assert_eq!(
            app.destination,
            PathBuf::from("dl"),
            "the session destination stays"
        );
        app.transfer = None;
        // A folder mirrors below the pane folder: {cwd}/{rel}/...
        app.handle(Msg::Key(key(KeyCode::Char('g'))));
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let dl = app.transfer.as_ref().unwrap();
        assert_eq!(dl.dst, cwd, "from-root names carry the rel themselves");
        app.transfer = None;
        // The put picker prefills with the pane folder.
        app.handle(Msg::Key(key(KeyCode::Char('p'))));
        let Mode::PickSource(picker) = app.mode.clone() else {
            panic!("p opens the picker");
        };
        assert_eq!(picker.buffer, cwd.display().to_string());
        app.handle(Msg::Key(key(KeyCode::Esc)));
        // On the repositories screen the repository mirrors: {cwd}/{repo}.
        app.tabs[0].screen = Screen::Repos;
        app.tabs[0].repo = None;
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let dl = app.transfer.as_ref().unwrap();
        assert_eq!(dl.dst, cwd.join("raw-main"));
        // v closes: the next download anchors to the session destination again.
        app.transfer = None;
        app.handle(Msg::Key(key(KeyCode::Char('v'))));
        app.tabs[0].screen = Screen::Tree;
        app.tabs[0].repo = app.tabs[0].repos.first().cloned();
        app.handle(Msg::Key(key(KeyCode::End)));
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        let dl = app.transfer.as_ref().unwrap();
        assert_eq!(
            dl.dst,
            PathBuf::from("dl"),
            "the anchor returns to the destination"
        );
        let Retry::Get { out, .. } = dl.retry.as_ref().unwrap() else {
            panic!("the file download retries as a get");
        };
        assert_eq!(out, &PathBuf::from("dl/README.txt"));
        let _ = root;
        let _ = rx;
    }

    #[tokio::test]
    async fn a_download_into_the_pane_folder_rereads_it() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        let (root, cwd) = open_pane(&mut app);
        app.handle(pane_entries_answer(&cwd));
        app.handle(Msg::Key(key(KeyCode::Char('l'))));
        let dir = root.path().to_path_buf();
        app.transfer = Some(Transfer {
            dir: Dir::Down,
            repo_url: "http://127.0.0.1:1/repository/raw-main/".into(),
            dst: dir.clone(),
            ..Transfer::default()
        });
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary {
            uploaded: 0,
            downloaded: 2,
            skipped: 0,
            removed: 0,
            failed: Vec::new(),
        }))));
        assert!(app.local.as_ref().unwrap().loading, "the pane rereads");
        match rx.recv().await.unwrap() {
            Msg::LocalEntries { cwd: token, res } => {
                assert_eq!(token, dir);
                assert!(res.is_ok(), "the real listing of the tempdir answers");
                app.handle(Msg::LocalEntries { cwd: token, res });
            }
            other => panic!("unexpected message: {other:?}"),
        }
        assert!(!app.local.as_ref().unwrap().loading);
        // A download elsewhere never triggers the reread.
        app.transfer.as_mut().unwrap().dst = PathBuf::from("/somewhere/else");
        app.handle(Msg::Dl(DlEv::Done(Ok(Summary::default()))));
        assert!(!app.local.as_ref().unwrap().loading);
    }

    #[tokio::test]
    async fn a_click_selects_in_the_pane_or_the_list() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("docs", EntryKind::Dir),
            ])],
            false,
        );
        let (root, cwd) = open_pane(&mut app);
        app.handle(pane_entries_answer(&cwd));
        app.local_area = Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
        };
        app.list_area = Rect {
            x: 40,
            y: 0,
            width: 30,
            height: 10,
        };
        let click = |col: u16, row: u16| {
            Msg::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: col,
                row,
                modifiers: KeyModifiers::empty(),
            })
        };
        // The left half selects in the pane and takes the focus.
        app.handle(click(5, 2));
        assert_eq!(app.local.as_ref().unwrap().cursor, 2);
        assert!(app.local_focus);
        // The right half selects in the list and hands the focus back.
        app.handle(click(45, 1));
        assert_eq!(app.tabs[0].tree_cursor, 1);
        assert!(!app.local_focus);
        // A click above the rows stays quiet.
        app.handle(click(45, 20));
        assert_eq!(app.tabs[0].tree_cursor, 1);
        let _ = root;
    }

    #[tokio::test]
    async fn download_plan_starts_the_transfer() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        // The real walk of the dead port lands: replace its result with the
        // plan the transfer would see.
        let Msg::Walk { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        app.handle(Msg::Walk {
            tab,
            gen,
            res: Ok(vec![ArtifactName::parse("app/lib.rs").unwrap()]),
        });
        assert_eq!(app.status, "plan: 1 file -> dl");
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { files, .. }) => assert_eq!(files, Some(1)),
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_stale_walk_never_starts_a_transfer() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        // A walk of an older generation lands: dropped, nothing transfers,
        // and the download it prepared does not stay "running" forever.
        app.handle(Msg::Walk {
            tab: 0,
            gen: app.tabs[0].gen - 1,
            res: Ok(vec![ArtifactName::parse("app/lib.rs").unwrap()]),
        });
        assert!(rx.try_recv().is_err(), "no download was spawned");
        assert!(app.transfer.is_none(), "the stale walk released the slot");
        assert!(!app.transfer_busy(), "the app is usable again");
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
        let Some(dl) = app.transfer.as_ref() else {
            panic!("the download panel exists");
        };
        assert!(matches!(dl.outcome, Some(DlOutcome::Failed(..))));
        assert!(
            !app.transfer_busy(),
            "a new download can start after the failure"
        );
        // The refusal speaks through the error modal, with a retry at hand.
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the walk refusal must open the error modal");
        };
        assert_eq!(modal.title, "download failed");
        assert!(modal.retry.is_some(), "the walk can run again");
        modal_retry_runs(&mut app);
        assert!(
            matches!(app.mode, Mode::Normal),
            "the modal closed on retry"
        );
        assert_eq!(app.status, "walking app/", "the retry rewalked");
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
        let Some(dl) = app.transfer.as_ref() else {
            panic!("the download panel exists");
        };
        assert!(matches!(dl.outcome, Some(DlOutcome::Failed(..))));
        assert!(
            !app.transfer_busy(),
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
        // Errors live in their modal now, never in the status line.
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the refusal must open the error modal");
        };
        assert_eq!(modal.title, "listing failed");
        assert!(modal.cause.contains("connection"), "{:?}", modal.cause);
        assert!(modal.retry.is_some(), "a transport failure can be retried");
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
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        match rx.recv().await.unwrap() {
            Msg::Dl(DlEv::Start { files, .. }) => assert_eq!(files, Some(1)),
            other => panic!("unexpected message: {other:?}"),
        }
        assert!(app.transfer_busy());
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
        assert_eq!(app.status, "transfer in flight: one at a time");
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
        // Two listings are in flight: the enter listing went stale when Left
        // bumped the generation, and the reload carries the fresh one.
        // Feed both in arrival order with the same canned entries: the stale
        // generation is ignored on apply, the fresh one lands deterministically.
        while app.pending > 0 {
            if let Msg::Entries { tab, gen, .. } = rx.recv().await.unwrap() {
                app.handle(Msg::Entries {
                    tab,
                    gen,
                    res: Ok(vec![
                        entry("app", EntryKind::Dir),
                        entry("README.txt", EntryKind::File),
                    ]),
                });
            }
        }
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Right)));
        // Right on a file opens the card now: no refusal class exists anymore.
        assert!(
            matches!(app.mode, Mode::Card(_)),
            "right on a file opens the card, got {:?}",
            app.status
        );
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
    async fn i_on_repos_opens_the_repository_card() {
        let (mut app, mut rx) = app_with(
            vec![tab_with(vec![
                repo("raw-main", "raw", "hosted"),
                repo("maven-central", "maven2", "proxy"),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('i'))));
        let Mode::Card(Card::Repo(card)) = app.mode.clone() else {
            panic!("i opens the repository card");
        };
        assert_eq!(card.repo.name, "raw-main");
        assert!(card.enterable);
        // The hidden format is named as such, no --all-formats needed to see it.
        app.handle(Msg::Key(key(KeyCode::Down)));
        // The card has the focus: movement keys do not reach the list.
        app.handle(Msg::Key(key(KeyCode::Esc)));
        app.handle(Msg::Key(key(KeyCode::Down)));
        app.handle(Msg::Key(key(KeyCode::Char('i'))));
        let Mode::Card(Card::Repo(card)) = app.mode.clone() else {
            panic!("i opens the card of the second row too");
        };
        assert_eq!(card.repo.name, "maven-central");
        assert!(!card.enterable);
        // q closes the card like esc: the overlay never quits the app.
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.quit);
        // `c` copies the url: the copied toast is the confirmation.
        app.handle(Msg::Key(key(KeyCode::Char('i'))));
        app.handle(Msg::Key(key(KeyCode::Char('c'))));
        assert_eq!(app.toasts.back().map(|t| t.text.as_str()), Some("copied"));
        assert!(rx.try_recv().is_err(), "the repo card spawns nothing");
    }

    #[tokio::test]
    async fn esc_from_the_tree_root_leaves_no_hidden_repository() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        // Open the repository, then walk back out with esc.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![entry("README.txt", EntryKind::File)]),
        });
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.tabs[0].screen, Screen::Repos);
        // The tab is virgin: no repo, no path, no entries.
        assert!(app.tabs[0].repo.is_none(), "no hidden repository state");
        assert!(app.tabs[0].path.is_empty());
        assert!(app.tabs[0].entries.is_empty());
        // The old mine is closed: D on the row opens its dialog instead of
        // silently downloading whatever repository was left in the tab.
        app.handle(Msg::Key(key(KeyCode::Char('D'))));
        assert!(
            matches!(app.mode, Mode::SaveAs(_)),
            "D is the explicit dialog"
        );
    }

    #[tokio::test]
    async fn q_in_the_servers_overlay_closes_it_instead_of_quitting() {
        let (mut app, _rx) = app_with(
            vec![tab_with(vec![repo("raw-main", "raw", "hosted")])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        assert_eq!(app.mode, Mode::Servers);
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.quit, "q never quits from an overlay");
        // Quitting stays a normal-screen act.
        app.handle(Msg::Key(key(KeyCode::Char('q'))));
        assert!(app.quit);
    }

    #[tokio::test]
    async fn the_filter_never_lingers_as_an_empty_zombie() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("core.txt", EntryKind::File),
            ])],
            false,
        );
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        app.handle(Msg::Key(key(KeyCode::Char('a'))));
        assert_eq!(app.status, "filter \"a\": 1 of 2 shown");
        // Backspace to empty closes the filter: no Some(""), no swallowed keys.
        app.handle(Msg::Key(key(KeyCode::Backspace)));
        assert!(app.tabs[0].filter.is_none(), "empty means off");
        assert!(!app.tabs[0].filter_edit, "the input closed with the filter");
        assert_eq!(app.status, hint_tree());
        assert_eq!(app.visible_len(), 2);
        // Enter on an empty buffer also just closes.
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert!(app.tabs[0].filter.is_none());
        // After the filter is applied the rest of the keyboard is back:
        // r, d, q do their own jobs instead of feeding the pattern.
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        for c in "co".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        app.handle(Msg::Key(key(KeyCode::Enter)));
        assert_eq!(app.tabs[0].filter.as_deref(), Some("co"));
        assert!(!app.tabs[0].filter_edit);
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert!(app.tabs[0].filter.is_none(), "esc clears the filter");
    }

    #[tokio::test]
    async fn refresh_keeps_the_filter_and_the_cursor_row() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![
                entry("app", EntryKind::Dir),
                entry("apple.txt", EntryKind::File),
                entry("zzz.txt", EntryKind::File),
            ])],
            false,
        );
        // Filter to the two apples and sit on the second one.
        app.handle(Msg::Key(key(KeyCode::Char('/'))));
        for c in "app".chars() {
            app.handle(Msg::Key(key(KeyCode::Char(c))));
        }
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Key(key(KeyCode::Down)));
        assert_eq!(
            app.tabs[0].selected_row().map(|r| r.name),
            Some("apple.txt".to_owned())
        );
        // r rereads; the landed listing keeps the filter and the cursor row.
        app.handle(Msg::Key(key(KeyCode::Char('r'))));
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Ok(vec![
                entry("apple.txt", EntryKind::File),
                entry("app", EntryKind::Dir),
                entry("zzz.txt", EntryKind::File),
            ]),
        });
        assert_eq!(
            app.tabs[0].filter.as_deref(),
            Some("app"),
            "the filter survives"
        );
        assert_eq!(
            app.tabs[0].selected_row().map(|r| r.name),
            Some("apple.txt".to_owned()),
            "the cursor returns to its row by name"
        );
    }

    #[tokio::test]
    async fn a_double_click_on_a_file_opens_the_card() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.list_area = Rect {
            x: 0,
            y: 1,
            width: 40,
            height: 5,
        };
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 1,
            modifiers: KeyModifiers::empty(),
        };
        app.handle(Msg::Mouse(click));
        app.handle(Msg::Mouse(click));
        assert!(
            matches!(app.mode, Mode::Card(_)),
            "the double click inspects, it never downloads"
        );
        assert!(app.transfer.is_none());
    }

    #[tokio::test]
    async fn the_error_modal_shows_cause_hint_and_full_text() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        let e = Error::Transport {
            url: "http://127.0.0.1:1/repository/raw-main/".into(),
            detail: "connection refused".into(),
        };
        let hint = e.hint();
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Err(e),
        });
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the listing failure opens the modal");
        };
        assert_eq!(modal.title, "listing failed");
        // One human sentence, no core spelling.
        assert_eq!(
            modal.cause,
            "the connection to 'http://127.0.0.1:1/repository/raw-main/' failed on the way"
        );
        // The facts name the tab's server and repository.
        assert!(modal
            .facts
            .contains(&("server".into(), "127.0.0.1:1".into())));
        assert!(modal
            .facts
            .contains(&("repository".into(), "raw-main".into())));
        // The hint stays verbatim, the full text carries both lines.
        assert_eq!(modal.hint, hint);
        let expected_full = match &hint {
            Some(h) => format!("error: transport: http://127.0.0.1:1/repository/raw-main/: connection refused\nhint: {h}"),
            None => "error: transport: http://127.0.0.1:1/repository/raw-main/: connection refused".to_owned(),
        };
        assert_eq!(modal.full, expected_full);
        // y copies the full text: the copied toast is the confirmation.
        app.handle(Msg::Key(key(KeyCode::Char('y'))));
        assert_eq!(app.toasts.back().map(|t| t.text.as_str()), Some("copied"));
        // r reruns the listing, esc closes.
        modal_retry_runs(&mut app);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.tabs[0].loading, "the retry refreshes the listing");
        // Esc alone closes without any retry.
        let e2 = Error::Transport {
            url: "http://x/".into(),
            detail: "refused".into(),
        };
        app.handle(Msg::Entries {
            tab: 0,
            gen: app.tabs[0].gen,
            res: Err(e2),
        });
        assert!(matches!(app.mode, Mode::Error { .. }));
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[tokio::test]
    async fn misuse_and_unsafe_names_hide_the_retry() {
        let (mut app, mut rx) = app_with(vec![tree_tab(vec![entry("app", EntryKind::Dir)])], false);
        app.handle(Msg::Key(key(KeyCode::Char('d'))));
        // Consume the dead port's transport answer, feed the grammar refusal.
        let Msg::Walk { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the walk result");
        };
        let e = Error::UnsafeName {
            name: "app/<bad>".into(),
            reason: "character outside the grammar".into(),
        };
        app.handle(Msg::Walk {
            tab,
            gen,
            res: Err(e),
        });
        let Mode::Error { modal, .. } = app.mode.clone() else {
            panic!("the walk refusal opens the modal");
        };
        assert!(modal.cause.contains("not a legal artifact path"));
        assert!(modal.retry.is_none(), "a grammar error cannot be retried");
        // Without a retry the modal only leaves through esc.
        app.handle(Msg::Key(key(KeyCode::Char('r'))));
        assert!(matches!(app.mode, Mode::Error { .. }));
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Normal);
        assert!(!app.transfer_busy(), "the failed walk released the slot");
    }

    #[tokio::test]
    async fn the_quit_guard_needs_two_presses_while_downloading() {
        let (mut app, _rx) = app_with(
            vec![tree_tab(vec![entry("README.txt", EntryKind::File)])],
            false,
        );
        app.transfer = Some(Transfer::default());
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
        // The connect goes through the live form, so the result is attached.
        app.mode = Mode::AddServer("http://x:2/".into());
        app.handle(Msg::Key(key(KeyCode::Enter)));
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
    async fn a_retry_while_flying_stays_detached_from_the_old_connect() {
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
        let server = ServerCfg::from_base("http://x:2/".to_owned());
        // A connect flies, the form cancels: the flying result is detached.
        app.mode = Mode::AddServer("http://x:2/".into());
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert!(!app.connect_open);
        // Retrying while the old connect is still flying must be refused
        // without arming the flag: the old result lands detached regardless.
        app.run_retry(Retry::Connect {
            server: server.clone(),
        });
        assert_eq!(app.status, "already connecting to a server");
        assert!(!app.connect_open, "the retry must not arm the flag");
        app.handle(Msg::Repos {
            tab: 1,
            server,
            connect: true,
            res: Ok(vec![repo("raw", "raw", "hosted")]),
        });
        assert_eq!(app.tabs.len(), 1, "the detached result opens no tab");
        let saved = crate::config::load(&path).unwrap();
        assert!(saved.servers.is_empty(), "the config stays untouched");
    }

    #[tokio::test]
    async fn the_card_downloads_a_nested_file_once() {
        let (mut app, mut rx) = app_with(
            vec![tree_tab(vec![
                entry("a", EntryKind::Dir),
                entry("c.txt", EntryKind::File),
            ])],
            false,
        );
        // Descend into `a`, then open the card of `c.txt`.
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let Msg::Entries { tab, gen, .. } = rx.recv().await.unwrap() else {
            panic!("expected the listing of a");
        };
        app.handle(Msg::Entries {
            tab,
            gen,
            res: Ok(vec![entry("c.txt", EntryKind::File)]),
        });
        app.handle(Msg::Key(key(KeyCode::Enter)));
        let Mode::Card(Card::File(card)) = app.mode.clone() else {
            panic!("enter opens the card of the nested file");
        };
        assert_eq!(card.rel, "a/c.txt");
        app.handle(Msg::Key(key(KeyCode::Char('s'))));
        let download = app.transfer.expect("the card download starts");
        let Retry::Get { url, out, .. } = download.retry.expect("the get retry") else {
            panic!("the card download retries as a get");
        };
        assert!(url.ends_with("/a/c.txt"), "{url}");
        assert!(out.ends_with("a/c.txt"), "{out:?}");
        assert!(!out.to_string_lossy().contains("a/a"), "{out:?}");
    }

    #[tokio::test]
    async fn a_cancelled_form_detaches_the_flying_connect() {
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
        // Submit, then cancel the form while the connect flies.
        app.mode = Mode::AddServer("http://x:2/".into());
        app.handle(Msg::Key(key(KeyCode::Enter)));
        app.handle(Msg::Key(key(KeyCode::Esc)));
        assert_eq!(app.mode, Mode::Servers, "esc goes back one layer");
        // The answer lands: no tab, no config write, the server is named.
        app.handle(Msg::Repos {
            tab: 1,
            server: ServerCfg::from_base("http://x:2/".to_owned()),
            connect: true,
            res: Ok(vec![repo("raw", "raw", "hosted")]),
        });
        assert_eq!(app.tabs.len(), 1, "no tab opens after the cancel");
        assert_eq!(app.status, "server http://x:2/ connected, press s to open");
        let saved = crate::config::load(&path).unwrap();
        assert!(saved.servers.is_empty(), "the config stays untouched");
        // The overlay knows the server, so `s` can still open it.
        assert!(app.servers.iter().any(|s| s.url == "http://x:2/"));
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
