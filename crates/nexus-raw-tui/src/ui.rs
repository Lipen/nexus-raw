//! Rendering: one frame per message, no timers.
//!
//! The frame is a header (tab bar), the body (repository list or tree, plus
//! the download panel while one runs), a status line and, on top, the active
//! overlay (help, servers, add-server form). The list geometry is written back
//! into the app so mouse clicks can hit-test rows.

use std::path::Path;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{
    fmt_bytes, App, Card, DestPicker, DlOutcome, ErrorModal, Mode, PickSource, SaveAsDialog,
    SaveAsField, Screen, ShaState, SizeState, Toast, Transfer,
};
use nexus_raw_core::{Dir, EntryKind};

/// The dim style for secondary and non-enterable text.
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The selected-row style.
fn selected() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Draws one frame: header, body, download panel, status, toasts, overlay.
/// The mutable borrow carries the list geometry back into the app.
pub fn draw(f: &mut Frame, app: &mut App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(f.area());
    draw_header(f, app, rows[0]);
    draw_body(f, app, rows[1]);
    draw_status(f, app, rows[2]);
    draw_toasts(f, app, f.area());
    draw_overlay(f, app, f.area());
}

/// The tab bar: one segment per open server, the active one reversed.
fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(
        " nxr-tui ",
        Style::default().add_modifier(Modifier::BOLD),
    )];
    if app.tabs.is_empty() {
        spans.push(Span::styled("no servers open", dim()));
    }
    for (i, tab) in app.tabs.iter().enumerate() {
        let mark = if tab.loading { "…" } else { "" };
        let label = format!(" {mark}{} ", tab.server.name);
        if i == app.tab {
            spans.push(Span::styled(
                label,
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ));
        } else {
            spans.push(Span::styled(label, dim()));
        }
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_body(f: &mut Frame, app: &mut App, area: Rect) {
    // The panel stays after the transfer ends: the outcome stays readable
    // until the next download replaces it.
    if app.transfer.is_none() {
        draw_list(f, app, area);
        return;
    }
    // Content: dst, plan, up to four transfers, one status line, plus borders.
    let active = app.transfer.as_ref().map_or(0, |dl| dl.active.len().min(4)) as u16;
    let panel_h = (3 + active + 2).min(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(panel_h)])
        .split(area);
    draw_list(f, app, chunks[0]);
    draw_transfer(f, app, chunks[1]);
}

/// The repository list or the tree, with the geometry written back for hit-testing.
fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(tab) = app.tabs.get(app.tab) else {
        let block = Block::default().borders(Borders::ALL).title("nxr-tui");
        f.render_widget(
            Paragraph::new("no tab: connect a server with s").block(block),
            area,
        );
        return;
    };
    let loading = if tab.loading { " · loading…" } else { "" };
    let (title, cursor, items): (String, usize, Vec<ListItem>) = match tab.screen {
        Screen::Repos => {
            let title = format!(
                "repositories on {} · {}{loading}",
                tab.server.name,
                tab.repos.len()
            );
            let items = tab
                .repos
                .iter()
                .map(|repo| ListItem::new(repo_line(tab.enterable(repo, app.all_formats), repo)))
                .collect();
            (title, tab.repos_cursor, items)
        }
        Screen::Tree => {
            let rows = tab.rows();
            let idx = tab.row_idx();
            let shown = idx.len();
            let filtered = tab.filter.as_ref().map_or(String::new(), |f| {
                format!(" · filter \"{f}\": {shown} kept")
            });
            let title = format!(
                "{} · {} of {}{filtered}{loading}",
                if tab.breadcrumb().is_empty() {
                    "tree".to_owned()
                } else {
                    tab.breadcrumb()
                },
                shown,
                rows.len()
            );
            let items = idx
                .into_iter()
                .filter_map(|i| rows.get(i))
                .map(|row| ListItem::new(tree_line(row)))
                .collect();
            (title, tab.tree_cursor, items)
        }
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    app.list_area = inner;
    app.list_state.select(Some(cursor));
    if items.is_empty() {
        let note = if tab.loading { "loading…" } else { "(none)" };
        f.render_widget(Paragraph::new(note).block(block), area);
        return;
    }
    let list = List::new(items)
        .block(block)
        .highlight_style(selected())
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut app.list_state);
}

/// One tree row: indentation, the expand marker for folders, then the name.
fn tree_line(row: &crate::app::Row) -> String {
    let indent = "  ".repeat(row.depth);
    match row.kind {
        EntryKind::Dir => format!(
            "{indent}{} {}/",
            if row.loading {
                "…"
            } else if row.expanded {
                "▾"
            } else {
                "▸"
            },
            row.name
        ),
        EntryKind::File => format!("{indent}  {}", row.name),
    }
}

/// One repository row: name, then the format and kind.
/// Non-enterable rows are dim, with the format as a badge.
fn repo_line(enterable: bool, repo: &nexus_raw_core::service::RepoInfo) -> Line<'static> {
    let base = format!("{:<14} ", repo.name);
    if enterable {
        return Line::from(format!("{base}{:<8} {}", repo.format, repo.kind));
    }
    let mut spans = vec![Span::styled(base, dim())];
    spans.push(Span::styled(
        format!("[{}] ", repo.format),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(repo.kind.clone(), dim()));
    Line::from(spans)
}

fn draw_transfer(f: &mut Frame, app: &App, area: Rect) {
    let Some(dl) = app.transfer.as_ref() else {
        return;
    };
    let title = match dl.dir {
        Dir::Up => "upload",
        Dir::Down => "download",
    };
    let mut lines = Vec::new();
    match dl.dir {
        Dir::Up => {
            lines.push(Line::from(format!("-> {}", dl.repo_url)));
            if let Some(src) = &dl.src {
                lines.push(Line::from(format!("from {}", src.display())));
            }
        }
        Dir::Down => {
            lines.push(Line::from(format!("-> {}", dl.dst.display())));
        }
    }
    match dl.files {
        Some(files) => match dl.dir {
            Dir::Up => lines.push(Line::from(format!("plan: {files} to upload"))),
            Dir::Down => {
                let unit = if files == 1 { "file" } else { "files" };
                lines.push(Line::from(format!("plan: {files} {unit}")));
            }
        },
        None => match dl.dir {
            Dir::Up => lines.push(Line::from("plan: scanning the folder")),
            Dir::Down => lines.push(Line::from("plan: walking the subtree")),
        },
    }
    if let Some((transfer, skip)) = dl.planned {
        let verb = match dl.dir {
            Dir::Up => "upload",
            Dir::Down => "download",
        };
        lines.push(Line::from(format!(
            "diff: {transfer} to {verb}, {skip} already complete"
        )));
    }
    for (name, (done, total)) in dl.active.iter().take(4) {
        lines.push(Line::from(format!(
            "  {name}  {}/{}",
            fmt_bytes(*done),
            total.map_or("?".to_owned(), fmt_bytes)
        )));
    }
    lines.push(outcome_line(dl));
    let panel = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(panel, area);
}

/// The last line of the download panel: retry note, running count or the outcome.
fn outcome_line(dl: &Transfer) -> Line<'static> {
    match &dl.outcome {
        Some(DlOutcome::Done(s)) => Line::from(Span::styled(
            format!(
                "done: downloaded {}, skipped {}, failed {}",
                s.downloaded,
                s.skipped,
                s.failed.len()
            ),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Some(DlOutcome::Failed(msg, hint)) => {
            let mut text = format!("failed: {msg}");
            if let Some(hint) = hint {
                text.push_str(&format!(" ({hint})"));
            }
            Line::from(Span::styled(text, Style::default().fg(Color::Red)))
        }
        None if dl.note.is_some() => {
            Line::from(Span::styled(dl.note.clone().unwrap_or_default(), dim()))
        }
        None => Line::from(Span::styled(
            format!(
                "transferring: {} finished, {} skipped",
                dl.finished, dl.skipped
            ),
            dim(),
        )),
    }
}

/// The last line: the transient status with the position on the right, plus
/// the constant destination indicator. Errors live in their own modal, never
/// here.
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let left = vec![Span::raw(format!(" {} ", app.status))];
    let right = self_status(app);
    // `·` is two UTF-8 bytes but one column: width math goes by chars.
    // Two spare columns keep a mid-word crop from touching the label.
    let left_w = area.width.saturating_sub(right.chars().count() as u16 + 2);
    // The label is right-aligned in the full width; the left part is clipped
    // to the remaining width, so the two can never overwrite each other.
    f.render_widget(Line::from(Span::styled(right, dim())).right_aligned(), area);
    f.render_widget(
        Line::from(left),
        Rect {
            width: left_w,
            ..area
        },
    );
}

/// The right side of the status line: tab position, server, cursor and the
/// session destination, always visible.
fn self_status(app: &App) -> String {
    let base = match app.tabs.get(app.tab) {
        Some(tab) => {
            let (cursor, len) = match tab.screen {
                Screen::Repos => (tab.repos_cursor, tab.repos.len()),
                Screen::Tree => (tab.tree_cursor, tab.row_idx().len()),
            };
            format!(
                "tab {}/{} · nav {} · {} · {}/{}",
                app.tab + 1,
                app.tabs.len(),
                app.nav.label(),
                tab.server.name,
                if len == 0 { 0 } else { cursor + 1 },
                len
            )
        }
        None => String::new(),
    };
    format!(
        "{base} · → {}",
        short_dst(&app.destination.display().to_string())
    )
}

/// The destination shortened for the corner: a leading ellipsis keeps the tail,
/// which is the part that distinguishes folders.
fn short_dst(dst: &str) -> String {
    const LIMIT: usize = 24;
    let chars = dst.chars().count();
    if chars <= LIMIT {
        dst.to_owned()
    } else {
        let tail: String = dst.chars().skip(chars + 1 - LIMIT).collect();
        format!("…{tail}")
    }
}

/// The toast stack: the youngest at the bottom, right above the status line,
/// right-aligned. Never takes focus, dies on its own timer.
fn draw_toasts(f: &mut Frame, app: &App, area: Rect) {
    let live: Vec<&Toast> = app.toasts.iter().rev().take(3).collect();
    let width = 60.min(area.width);
    for (i, toast) in live.iter().enumerate() {
        let y = area.y + area.height.saturating_sub(2 + i as u16);
        let row = Rect {
            x: area.x + area.width.saturating_sub(width),
            y,
            width,
            height: 1,
        };
        f.render_widget(
            Line::from(Span::styled(
                format!(" {} ", toast.text),
                Style::default().fg(Color::Green),
            ))
            .right_aligned(),
            row,
        );
    }
}

fn draw_overlay(f: &mut Frame, app: &mut App, area: Rect) {
    match app.mode.clone() {
        Mode::Normal => {}
        Mode::Help => help_overlay(f, app, area),
        Mode::Servers => servers_overlay(f, app, area),
        Mode::AddServer(buffer) => add_server_overlay(f, &buffer, area),
        Mode::Dest(picker) => dest_picker_overlay(f, app, &picker, area),
        Mode::PickSource(picker) => source_picker_overlay(f, app, &picker, area),
        Mode::Card(card) => card_overlay(f, &card, area),
        Mode::SaveAs(dialog) => save_as_overlay(f, &dialog, area),
        Mode::Error(modal) => error_overlay(&modal, area, f),
    }
}

fn help_overlay(f: &mut Frame, app: &App, area: Rect) {
    let config = app.config_path.as_ref().map_or_else(
        || "config: none yet (--init-config writes one)".to_owned(),
        |p| format!("config: {}", p.display()),
    );
    let screen_section: Vec<String> = match app.tabs.get(app.tab).map(|t| t.screen) {
        Some(Screen::Repos) => vec![
            "here: repositories".to_owned(),
            "  enter, right      open a raw repository".to_owned(),
            "  d                 download the whole repository".to_owned(),
            "  D                 download it as, with a name of your own".to_owned(),
            "  i                 card with the repository details".to_owned(),
            String::new(),
        ],
        Some(Screen::Tree) => vec![
            "here: the tree of a repository".to_owned(),
            "  enter on a folder descend, on a file open its card".to_owned(),
            "  right             enter mode: the same as enter".to_owned(),
            "                    expand mode: fold or unfold inline".to_owned(),
            "  left              enter mode: back up. expand mode: fold or jump".to_owned(),
            "  d                 download the selection (folder: subtree)".to_owned(),
            "  D                 download the current folder as".to_owned(),
            "  p                 upload a local folder into this position".to_owned(),
            "  /                 filter, type to narrow, backspace to empty closes".to_owned(),
            String::new(),
        ],
        None => vec![String::new()],
    };
    let mut lines: Vec<String> = vec![
        "navigation".to_owned(),
        "  up/down, k/j      move the selection".to_owned(),
        "  pgup/pgdown       move by page".to_owned(),
        "  home/end, g/G     jump to the first/last row".to_owned(),
        "  tab / backtab     next/previous server tab, 1-9 jump".to_owned(),
        "  esc, backspace    up one layer: filter, folder, repositories".to_owned(),
        String::new(),
    ];
    lines.extend(screen_section);
    lines.extend([
        "destination".to_owned(),
        "  o                 pick the download folder of this session".to_owned(),
        "                    files land as {destination}/{path}, a repository".to_owned(),
        "                    as {destination}/{repo}, shown at the bottom right".to_owned(),
        String::new(),
        "card".to_owned(),
        "  s                 download into the destination".to_owned(),
        "  o                 change the destination, back to the card".to_owned(),
        "  c                 copy the url".to_owned(),
        "  esc, enter, q     close".to_owned(),
        String::new(),
        "actions".to_owned(),
        "  e                 toggle enter/expand navigation (saved to the config)".to_owned(),
        "  r                 refresh, keeps the filter and the cursor".to_owned(),
        "  s                 servers: switch or add, the add saves a preset".to_owned(),
        "  a                 (in servers) open the add-server form".to_owned(),
        "  ?                 this help".to_owned(),
        "  q, ctrl-c         quit from the normal screen (twice while".to_owned(),
        "                    a download runs); in an overlay q only closes it".to_owned(),
        String::new(),
        "mouse: wheel scrolls, click selects, double click opens".to_owned(),
        config,
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" nxr-tui keys ");
    let inner = block.inner(area);
    let w = inner.width.min(74);
    let h = (lines.len() as u16 + 2).min(inner.height);
    let popup = centered(area, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .block(block)
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn servers_overlay(f: &mut Frame, app: &mut App, area: Rect) {
    let items: Vec<ListItem> = app
        .servers
        .iter()
        .map(|s| {
            let open = app.tabs.iter().any(|t| t.server.url == s.url);
            let line = if open {
                Line::from(format!("{}  {} (open)", s.name, s.url))
            } else {
                Line::from(Span::styled(format!("{}  {}", s.name, s.url), dim()))
            };
            ListItem::new(line)
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" servers · enter: open · a: add · esc: close ");
    let popup = centered(
        area,
        60.min(area.width),
        (app.servers.len() as u16 + 2 + 1).min(area.height),
    );
    f.render_widget(Clear, popup);
    if items.is_empty() {
        f.render_widget(Paragraph::new("(no servers known)").block(block), popup);
        return;
    }
    let mut state = ratatui::widgets::ListState::default().with_selected(Some(app.servers_cursor));
    let list = List::new(items)
        .block(block)
        .highlight_style(selected())
        .highlight_symbol("> ");
    f.render_stateful_widget(list, popup, &mut state);
}

fn add_server_overlay(f: &mut Frame, buffer: &str, area: Rect) {
    let popup = centered(area, 60.min(area.width), 5);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" add server · enter: connect · esc: cancel ");
    let lines = vec![
        Line::from("server root URL, credentials come from -u or the env:"),
        Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{buffer}█")),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(block), popup);
}

/// A rect of `w`×`h` centered in `area`.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}

/// The destination picker: the buffer, the resolved preview, the rules.
fn dest_picker_overlay(f: &mut Frame, app: &App, picker: &DestPicker, area: Rect) {
    let resolved = app
        .resolve_path(Path::new(picker.buffer.trim()))
        .display()
        .to_string();
    let lines = vec![
        Line::from("folder downloads land in, relative to where nxr-tui started:"),
        Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{}█", picker.buffer)),
        ]),
        Line::from(Span::styled(format!("→ {resolved}"), dim())),
    ];
    let popup = centered(area, 64.min(area.width), 5);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" destination · enter: set · esc: cancel "),
        ),
        popup,
    );
}

/// The source picker of a put: the buffer, the resolved preview, the position.
fn source_picker_overlay(f: &mut Frame, app: &App, picker: &PickSource, area: Rect) {
    let resolved = app
        .resolve_path(Path::new(picker.buffer.trim()))
        .display()
        .to_string();
    let pos = app
        .tabs
        .get(app.tab)
        .map(|t| t.breadcrumb())
        .unwrap_or_default();
    let lines = vec![
        Line::from(format!("local folder to upload into {pos}:")),
        Line::from(vec![
            Span::styled("> ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{}█", picker.buffer)),
        ]),
        Line::from(Span::styled(format!("→ {resolved}"), dim())),
    ];
    let popup = centered(area, 64.min(area.width), 5);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" put source · enter: upload · esc: cancel "),
        ),
        popup,
    );
}

/// The card over a repository or a file.
fn card_overlay(f: &mut Frame, card: &Card, area: Rect) {
    let (title, lines): (String, Vec<Line>) = match card {
        Card::Repo(c) => {
            let enterable = if c.enterable {
                "yes".to_owned()
            } else {
                format!(
                    "no, the {} filter hides it (--all-formats lifts it)",
                    c.repo.format
                )
            };
            let body = vec![
                Line::from(format!("format: {} ({})", c.repo.format, c.repo.kind)),
                Line::from(format!("openable: {enterable}")),
                Line::from(format!("url: {}", c.repo.url)),
                Line::from(Span::styled(
                    "c: copy url · esc, enter, q: close".to_owned(),
                    dim(),
                )),
            ];
            (format!(" {}", c.repo.name), body)
        }
        Card::File(c) => {
            let size = match &c.size {
                SizeState::Pending => "…".to_owned(),
                SizeState::Unknown => "unknown (the server sent no Content-Length)".to_owned(),
                SizeState::Known(n) => fmt_bytes(*n),
                SizeState::Failed(e) => format!("head failed: {e}"),
            };
            let sha = match &c.sha {
                ShaState::Pending => "…".to_owned(),
                ShaState::Absent => "none (no .sha256 marker)".to_owned(),
                ShaState::Hex(hex) => hex.clone(),
                ShaState::Failed(e) => format!("marker failed: {e}"),
            };
            let body = vec![
                Line::from(format!("path: {} (in {})", c.rel, c.repo.name)),
                Line::from(format!("size: {size}")),
                Line::from(format!("sha256: {sha}")),
                Line::from(format!("url: {}", c.url)),
                Line::from(Span::styled(
                    "s: download · o: destination · c: copy url · esc, enter, q: close".to_owned(),
                    dim(),
                )),
            ];
            (format!(" {}", c.name), body)
        }
    };
    let width = 74.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered(area, width, height);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        popup,
    );
}

/// The download-as dialog: two fields, the live preview of the target path.
fn save_as_overlay(f: &mut Frame, dialog: &SaveAsDialog, area: Rect) {
    let field = |active: bool| {
        if active {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        }
    };
    let cursor = |active: bool| if active { "█" } else { "" };
    let lines = vec![
        Line::styled(
            format!(
                "folder  {}{}",
                dialog.dir,
                cursor(dialog.field == SaveAsField::Dir)
            ),
            field(dialog.field == SaveAsField::Dir),
        ),
        Line::styled(
            format!(
                "name    {}{}",
                dialog.name,
                cursor(dialog.field == SaveAsField::Name)
            ),
            field(dialog.field == SaveAsField::Name),
        ),
        Line::from(Span::styled(
            format!("→ {}/{}", dialog.dir, dialog.name),
            dim(),
        )),
        Line::from(Span::styled(
            "tab: field · enter: next or start · esc: cancel".to_owned(),
            dim(),
        )),
    ];
    let popup = centered(area, 64.min(area.width), 6);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" download as "),
        ),
        popup,
    );
}

/// The error modal: cause, facts, next steps, full text.
fn error_overlay(modal: &ErrorModal, area: Rect, f: &mut Frame) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            modal.cause.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(String::new()),
    ];
    for (key, value) in &modal.facts {
        lines.push(Line::from(vec![
            Span::styled(format!("{key}: "), dim()),
            Span::raw(value.clone()),
        ]));
    }
    if !modal.facts.is_empty() {
        lines.push(Line::from(String::new()));
    }
    if let Some(hint) = &modal.hint {
        lines.push(Line::from(Span::styled("next steps:", dim())));
        lines.push(Line::from(hint.clone()));
        lines.push(Line::from(String::new()));
    }
    lines.push(Line::from(Span::styled("full text (y copies):", dim())));
    for row in modal.full.lines() {
        lines.push(Line::from(Span::styled(row.to_owned(), dim())));
    }
    let mut footer = "esc, enter: close · y: copy".to_owned();
    if modal.retry.is_some() {
        footer.push_str(" · r: retry");
    }
    lines.push(Line::from(String::new()));
    lines.push(Line::from(Span::styled(footer, dim())));
    let width = 74.min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered(area, width, height);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", modal.title)),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}
