//! Rendering: one frame per message, no timers.
//!
//! The frame is a header (tab bar), the body (repository list or tree, plus
//! the download panel while one runs), a status line and, on top, the active
//! overlay (help, servers, add-server form). The list geometry is written back
//! into the app so mouse clicks can hit-test rows.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{fmt_bytes, App, DlOutcome, Download, Mode, Screen};
use nexus_raw_core::EntryKind;

/// The dim style for secondary and non-enterable text.
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The selected-row style.
fn selected() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

/// Draws one frame: header, body, download panel, status, overlay.
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
    if app.download.is_none() {
        draw_list(f, app, area);
        return;
    }
    // Content: dst, plan, up to four transfers, one status line, plus borders.
    let active = app.download.as_ref().map_or(0, |dl| dl.active.len().min(4)) as u16;
    let panel_h = (3 + active + 2).min(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(panel_h)])
        .split(area);
    draw_list(f, app, chunks[0]);
    draw_download(f, app, chunks[1]);
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
            let idx = tab.filter_idx();
            let shown = idx.len();
            let filtered = tab
                .filter
                .as_ref()
                .map_or(String::new(), |f| format!(" · filter {f:?}: {shown} kept"));
            let title = format!(
                "{} · {} of {}{filtered}{loading}",
                if tab.breadcrumb().is_empty() {
                    "tree".to_owned()
                } else {
                    tab.breadcrumb()
                },
                shown,
                tab.entries.len()
            );
            let items = idx
                .into_iter()
                .filter_map(|i| tab.entries.get(i))
                .map(|entry| {
                    ListItem::new(match entry.kind {
                        EntryKind::Dir => format!("{}/", entry.name),
                        EntryKind::File => entry.name.clone(),
                    })
                })
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

fn draw_download(f: &mut Frame, app: &App, area: Rect) {
    let Some(dl) = app.download.as_ref() else {
        return;
    };
    let mut lines = Vec::new();
    lines.push(Line::from(format!("-> {}", dl.dst.display())));
    match dl.files {
        Some(files) => {
            let unit = if files == 1 { "file" } else { "files" };
            lines.push(Line::from(format!("plan: {files} {unit}")));
        }
        None => lines.push(Line::from("plan: walking the subtree")),
    }
    if let Some((download, skip)) = dl.planned {
        lines.push(Line::from(format!(
            "diff: {download} to download, {skip} already complete"
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
    let panel =
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("download"));
    f.render_widget(panel, area);
}

/// The last line of the download panel: retry note, running count or the outcome.
fn outcome_line(dl: &Download) -> Line<'static> {
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

/// The last line: error, info, or the status with the position on the right.
/// The left part is clipped to the width the right part does not use.
fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let left = if let Some((msg, hint)) = &app.error {
        let mut spans = vec![Span::styled(
            format!(" error: {msg} "),
            Style::default().fg(Color::Red),
        )];
        if let Some(hint) = hint {
            spans.push(Span::styled(format!(" {hint}"), dim()));
        }
        spans
    } else if let Some(info) = &app.info {
        vec![Span::styled(format!(" {info} "), dim())]
    } else {
        vec![Span::raw(format!(" {} ", app.status))]
    };
    let right = self_status(app);
    let left_w = area.width.saturating_sub(right.len() as u16);
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

/// The right side of the status line: tab position, server and cursor.
fn self_status(app: &App) -> String {
    let Some(tab) = app.tabs.get(app.tab) else {
        return String::new();
    };
    let (cursor, len) = match tab.screen {
        Screen::Repos => (tab.repos_cursor, tab.repos.len()),
        Screen::Tree => (tab.tree_cursor, tab.filter_idx().len()),
    };
    format!(
        "tab {}/{} · {} · {}/{}",
        app.tab + 1,
        app.tabs.len(),
        tab.server.name,
        if len == 0 { 0 } else { cursor + 1 },
        len
    )
}

fn draw_overlay(f: &mut Frame, app: &mut App, area: Rect) {
    match app.mode.clone() {
        Mode::Normal => {}
        Mode::Help => help_overlay(f, area),
        Mode::Servers => servers_overlay(f, app, area),
        Mode::AddServer(buffer) => add_server_overlay(f, &buffer, area),
    }
}

fn help_overlay(f: &mut Frame, area: Rect) {
    const LINES: &[&str] = &[
        "navigation",
        "  up/down, k/j      move the selection",
        "  pgup/pgdown       move by page",
        "  home/end, g/G     jump to the first/last row",
        "  tab / backtab     next/previous server tab",
        "  1-9               jump to the n-th tab",
        "  esc, backspace    up one level, then back to repositories",
        "",
        "actions",
        "  enter             open a folder or repository, download a file",
        "  d                 download the selected entry (folder: subtree)",
        "  D                 download the whole current directory",
        "  r                 refresh the current listing",
        "  i                 info on the selected entry (HEAD size)",
        "  /                 filter the tree: type to narrow, esc clears",
        "  s                 servers: switch or add, the add saves a preset",
        "  a                 (in servers) open the add-server form",
        "  ?                 this help",
        "  q, ctrl-c         quit (twice while a download runs)",
        "",
        "mouse: wheel scrolls, click selects, double click opens",
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" nxr-tui keys ");
    let inner = block.inner(area);
    let w = inner.width.min(64);
    let h = (LINES.len() as u16 + 2).min(inner.height);
    let popup = centered(area, w, h);
    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(LINES.iter().map(|l| Line::from(*l)).collect::<Vec<_>>())
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
