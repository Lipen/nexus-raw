//! Event rendering: human console output and the renderer task plumbing.

mod json;

use std::io::IsTerminal;

use nexus_raw_core::{Dir, Event};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// How the event stream is rendered.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Mode {
    Human { quiet: bool, verbose: bool },
    Json,
}

impl Mode {
    pub(crate) fn from_flags(json: bool, quiet: bool, verbose: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Human { quiet, verbose }
        }
    }
}

/// Owns the event channel and the task draining it.
///
/// The sender is cloned into the core.
/// Dropping the session's own sender after the operation closes the channel and lets the renderer finish.
pub(crate) struct Session {
    tx: UnboundedSender<Event>,
    renderer: tokio::task::JoinHandle<()>,
}

impl Session {
    pub(crate) fn start(mode: Mode) -> Self {
        let (tx, rx) = unbounded_channel();
        let renderer = tokio::spawn(run(rx, mode));
        Self { tx, renderer }
    }

    pub(crate) fn sender(&self) -> UnboundedSender<Event> {
        self.tx.clone()
    }

    /// Close the channel and wait until every queued event is rendered.
    pub(crate) async fn finish(self) {
        drop(self.tx);
        let _ = self.renderer.await;
    }
}

async fn run(mut rx: UnboundedReceiver<Event>, mode: Mode) {
    let mut progress: Option<u16> = None;
    while let Some(ev) = rx.recv().await {
        match mode {
            Mode::Human { quiet, verbose } => human(&ev, quiet, verbose, &mut progress),
            Mode::Json => json::line(&ev),
        }
    }
    clear_progress(&progress);
}

/// Erase an in-place progress line, if one is drawn.
fn clear_progress(progress: &Option<u16>) {
    if let Some(w) = progress {
        let blank = " ".repeat(*w as usize);
        eprint!("\r{blank}\r");
    }
}

/// Human console rendering.
///
/// `-q` keeps only the final summary.
/// `-v` adds transfer starts and plan names.
/// Byte-progress draws one in-place stderr line when stderr is a terminal.
fn human(ev: &Event, quiet: bool, verbose: bool, progress: &mut Option<u16>) {
    // Any non-progress line starts clean, so the in-place drawing never smears.
    if !matches!(ev, Event::ArtifactBytes { .. }) {
        clear_progress(progress);
    }
    match ev {
        Event::Plan {
            upload,
            download,
            skip,
        } => {
            if quiet {
                return;
            }
            println!(
                "plan: {} to upload, {} to download, {} up to date",
                upload.len(),
                download.len(),
                skip.len()
            );
            if verbose {
                for name in upload {
                    println!("↑ {name}");
                }
                for name in download {
                    println!("↓ {name}");
                }
                for name in skip {
                    println!("○ {name}");
                }
            }
        }
        Event::ArtifactStarted { name, total, .. } => {
            if verbose && !quiet {
                println!(
                    "→ {name} ({})",
                    (*total).map_or("?".into(), |t| t.to_string())
                );
            }
        }
        Event::ArtifactBytes {
            name,
            dir,
            done,
            total,
        } => {
            // The in-place progress line lives on stderr and only on a terminal:
            // stdout stays the event stream, pipes stay clean.
            if quiet || !std::io::stderr().is_terminal() {
                return;
            }
            let arrow = match dir {
                Dir::Up => "↑",
                Dir::Down => "↓",
            };
            let line = match total {
                Some(t) if *t > 0 => {
                    format!("{arrow} {name} {done}/{t} ({}%)", done * 100 / t)
                }
                _ => format!("{arrow} {name} {done} bytes"),
            };
            let pad = (*progress).map_or(0, |w| (w as usize).saturating_sub(line.len()));
            eprint!("\r{line}{}", " ".repeat(pad));
            *progress = Some(line.len() as u16);
        }
        Event::ArtifactDone {
            name, dir, skipped, ..
        } => {
            if quiet {
                return;
            }
            let mark = match (skipped, dir) {
                (true, _) => "○",
                (false, Dir::Up) => "↑",
                (false, Dir::Down) => "↓",
            };
            let tail = if *skipped { "skipped" } else { "ok" };
            println!("{mark} {name} {tail}");
        }
        Event::Retrying {
            name,
            attempt,
            reason,
        } => {
            if !quiet {
                println!("↻ {name}: retry {attempt} ({reason})");
            }
        }
        Event::Removing { name } => {
            if verbose && !quiet {
                println!("× {name}");
            }
        }
        Event::Removed { name } => {
            if quiet {
                return;
            }
            println!("× {name} removed");
        }
        Event::Missing { name } => {
            if quiet {
                return;
            }
            println!("○ {name} missing");
        }
        Event::Summary(s) => {
            if s.removed > 0 {
                println!("removed {}, skipped {}", s.removed, s.skipped);
            } else {
                println!(
                    "uploaded {}, downloaded {}, skipped {}",
                    s.uploaded, s.downloaded, s.skipped
                );
            }
            if !s.failed.is_empty() {
                println!("failed: {}", s.failed.join(", "));
            }
        }
    }
}
