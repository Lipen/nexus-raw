//! Rendering: one frame per message, no timers.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{App, DlOutcome, Screen};

/// The dim style for secondary text.
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// The selected-row style.
fn selected() -> Style {
    Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED)
}

/// Draws one frame: header, body, progress panel, status bar.
pub fn draw(f: &mut Frame, app: &App) {
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
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" nxr-tui ", Style::default().add_modifier(Modifier::BOLD))];
    if let Some(repo) = &app.current_repo {
        spans.push(Span::styled(repo.repo.name.clone(), dim()));
        if let Some(version) = &app.current_version {
            spans.push(Span::styled(" > ", dim()));
            spans.push(Span::styled(version.clone(), dim()));
        }
    }
    let servers = app
        .servers
        .iter()
        .map(|s| s.trim_end_matches('/'))
        .collect::<Vec<_>>()
        .join(" ");
    spans.push(Span::styled(format!("  {servers}"), dim()));
    f.render_widget(Line::from(spans), area);
}

fn draw_body(f: &mut Frame, app: &App, area: Rect) {
    let Some(dl) = app.download.as_ref() else {
        draw_list(f, app, area);
        return;
    };
    // Content: dst, plan, up to four transfers, one status line, plus borders.
    let active = dl.active.len().min(4) as u16;
    let panel_h = (3 + active + 2).min(area.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(panel_h)])
        .split(area);
    draw_list(f, app, chunks[0]);
    draw_download(f, app, chunks[1]);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let (title, items): (&str, Vec<String>) = match app.screen {
        Screen::Repos => (
            "repositories",
            app.repos
                .iter()
                .map(|row| {
                    let server = app
                        .servers
                        .get(row.server)
                        .map(|s| s.trim_end_matches('/'))
                        .unwrap_or("?");
                    format!(
                        "{server}  {:<12} {:<7} {}",
                        row.repo.name, row.repo.format, row.repo.kind
                    )
                })
                .collect(),
        ),
        Screen::Versions => (
            "versions",
            app.versions.iter().cloned().collect(),
        ),
        Screen::Assets => (
            "objects",
            app.assets.iter().map(|n| n.as_str().to_owned()).collect(),
        ),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    if items.is_empty() {
        let note = if app.pending > 0 { "loading…" } else { "(none)" };
        f.render_widget(Paragraph::new(note).block(block), area);
        return;
    }
    let cursor = match app.screen {
        Screen::Repos => app.repos_cursor,
        Screen::Versions => app.versions_cursor,
        Screen::Assets => app.assets_cursor,
    };
    let mut state = ListState::default();
    state.select(Some(cursor));
    let list = List::new(items.into_iter().map(ListItem::new).collect::<Vec<_>>())
        .block(block)
        .highlight_style(selected())
        .highlight_symbol("> ");
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_download(f: &mut Frame, app: &App, area: Rect) {
    let Some(dl) = app.download.as_ref() else {
        return;
    };
    let mut lines = Vec::new();
    lines.push(Line::from(format!("-> {}", dl.dst.display())));
    match dl.planned {
        Some((download, skip)) => {
            lines.push(Line::from(format!(
                "plan: {download} to download, {skip} skipped"
            )));
        }
        None => lines.push(Line::from("plan: waiting for the diff")),
    }
    for (name, (done, total)) in dl.active.iter().take(4) {
        lines.push(Line::from(format!(
            "  {name}  {}/{}",
            fmt_bytes(*done),
            total.map_or("?".into(), |t| fmt_bytes(t))
        )));
    }
    match &dl.outcome {
        Some(DlOutcome::Done(s)) => {
            lines.push(Line::from(Span::styled(
                format!(
                    "done: downloaded {}, skipped {}, failed {}",
                    s.downloaded,
                    s.skipped,
                    s.failed.len()
                ),
                Style::default().add_modifier(Modifier::BOLD),
            )));
        }
        Some(DlOutcome::Failed(msg, hint)) => {
            let mut text = format!("failed: {msg}");
            if let Some(hint) = hint {
                text.push_str(&format!(" ({hint})"));
            }
            lines.push(Line::from(Span::styled(
                text,
                Style::default().fg(ratatui::style::Color::Red),
            )));
        }
        None if dl.note.is_some() => {
            lines.push(Line::from(Span::styled(
                dl.note.clone().unwrap_or_default(),
                dim(),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                format!("transferring: {} finished, {} skipped", dl.finished, dl.skipped),
                dim(),
            )));
        }
    }
    let panel = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("download"));
    f.render_widget(panel, area);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some((msg, hint)) = &app.error {
        let mut spans = vec![Span::styled(
            format!(" error: {msg} "),
            Style::default().fg(ratatui::style::Color::Red),
        )];
        if let Some(hint) = hint {
            spans.push(Span::styled(format!(" {hint}"), dim()));
        }
        Line::from(spans)
    } else if app
        .download
        .as_ref()
        .is_some_and(crate::app::Download::is_running)
    {
        Line::from(Span::styled(
            format!(" downloading… {} ", app.status),
            Style::default().fg(ratatui::style::Color::Yellow),
        ))
    } else {
        Line::from(format!(" {} ", app.status))
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Formats a byte count for the progress line.
fn fmt_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    let n = n as f64;
    if n >= MIB {
        format!("{:.1} MiB", n / MIB)
    } else if n >= KIB {
        format!("{:.1} KiB", n / KIB)
    } else {
        format!("{n} B")
    }
}
