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
    let mut progress = Progress::default();
    let paint = Paint::new(human_colors());
    while let Some(ev) = rx.recv().await {
        match mode {
            Mode::Human { quiet, verbose } => human(&ev, quiet, verbose, &mut progress, &paint),
            Mode::Json => json::line(&ev),
        }
    }
    progress.clear();
}

/// ANSI painting of the human lines.
///
/// Colors cost bytes, so the lines that tests and pipes read stay byte-identical:
/// [`Paint::new`] is handed a [`human_colors`] verdict, and only a terminal turns it on.
#[derive(Debug, Clone, Copy)]
struct Paint {
    enabled: bool,
}

impl Paint {
    fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    fn wrap(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    fn bold(&self, text: &str) -> String {
        self.wrap("1", text)
    }

    fn dim(&self, text: &str) -> String {
        self.wrap("2", text)
    }

    fn green(&self, text: &str) -> String {
        self.wrap("32", text)
    }

    fn yellow(&self, text: &str) -> String {
        self.wrap("33", text)
    }

    fn red(&self, text: &str) -> String {
        self.wrap("31", text)
    }
}

/// Whether the human lines may carry ANSI.
///
/// Only an interactive stdout gets them: pipes and files stay byte-clean for tests and scripts.
/// `NO_COLOR` (any non-empty value) and a dumb terminal always win.
fn human_colors() -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var_os("TERM").is_some_and(|v| v == "dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// A byte count for humans: B, then KiB, MiB or GiB with one decimal.
fn human_bytes(n: u64) -> String {
    const KIB: f64 = 1024.0;
    let f = n as f64;
    if f < KIB {
        format!("{n} B")
    } else if f < KIB * KIB {
        format!("{:.1} KiB", f / KIB)
    } else if f < KIB * KIB * KIB {
        format!("{:.1} MiB", f / (KIB * KIB))
    } else {
        format!("{:.1} GiB", f / (KIB * KIB * KIB))
    }
}

/// Live transfer state behind the single in-place progress line.
///
/// One line, not one per artifact: parallel workers would otherwise smear each
/// other's counters across the terminal, and the reader would see neither.
#[derive(Default)]
struct Progress {
    /// Planned transfers: the plan's upload plus download lists.
    files_total: usize,
    /// Settled artifacts, skips included.
    files_done: usize,
    /// The artifact of the most recent byte event: name, direction, done, total.
    current: Option<(String, Dir, u64, Option<u64>)>,
    /// Visible width of the last drawn line, for erasing it.
    width: u16,
}

impl Progress {
    fn note_bytes(&mut self, name: &str, dir: Dir, done: u64, total: Option<u64>) {
        self.current = Some((name.to_owned(), dir, done, total));
    }

    /// Erase the in-place line, if one is drawn.
    fn clear(&mut self) {
        if self.width == 0 {
            return;
        }
        eprint!("\r{}\r", " ".repeat(self.width as usize));
        self.width = 0;
    }

    /// Redraw the in-place line: overall file count plus the freshest artifact.
    ///
    /// The line carries no ANSI: erasing it relies on a width that escapes would inflate.
    fn draw(&mut self) {
        if !std::io::stderr().is_terminal() {
            return;
        }
        let Some((name, dir, done, total)) = self.current.clone() else {
            return;
        };
        let arrow = match dir {
            Dir::Up => "↑",
            Dir::Down => "↓",
        };
        let mut line = if self.files_total > 0 {
            format!("{arrow} {}/{} files", self.files_done, self.files_total)
        } else {
            arrow.to_owned()
        };
        line.push_str(" · ");
        line.push_str(&name);
        match total {
            Some(t) if t > 0 => line.push_str(&format!(
                " {}/{} ({}%)",
                human_bytes(done),
                human_bytes(t),
                done * 100 / t
            )),
            _ => line.push_str(&format!(" {}", human_bytes(done))),
        }
        let pad = (self.width as usize).saturating_sub(line.len());
        eprint!("\r{line}{}", " ".repeat(pad));
        self.width = line.len().min(u16::MAX as usize) as u16;
    }
}

/// Human console rendering.
///
/// `-q` keeps only the final summary.
/// `-v` adds transfer starts and plan names.
/// Live byte progress draws one in-place stderr line when stderr is a terminal.
fn human(ev: &Event, quiet: bool, verbose: bool, prog: &mut Progress, paint: &Paint) {
    // Any non-progress line starts clean, so the in-place drawing never smears.
    if !matches!(ev, Event::ArtifactBytes { .. }) {
        prog.clear();
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
                paint.bold(&upload.len().to_string()),
                paint.bold(&download.len().to_string()),
                paint.bold(&skip.len().to_string())
            );
            prog.files_total = upload.len() + download.len();
            if verbose {
                for name in upload {
                    println!("{} {name}", paint.dim("↑"));
                }
                for name in download {
                    println!("{} {name}", paint.dim("↓"));
                }
                for name in skip {
                    println!("{} {name}", paint.dim("○"));
                }
            }
        }
        Event::ArtifactStarted { name, dir, total } => {
            if verbose && !quiet {
                let arrow = match dir {
                    Dir::Up => "↑",
                    Dir::Down => "↓",
                };
                let size = (*total).map_or_else(|| "?".to_owned(), human_bytes);
                println!("{} {name} {}", paint.dim(arrow), paint.dim(&size));
            }
        }
        Event::ArtifactBytes {
            name,
            dir,
            done,
            total,
        } => {
            if quiet {
                return;
            }
            prog.note_bytes(name, *dir, *done, *total);
            prog.draw();
        }
        Event::ArtifactDone {
            name, dir, skipped, ..
        } => {
            if quiet {
                return;
            }
            prog.files_done += 1;
            let text = match (skipped, dir) {
                (true, _) => format!("○ {name} skipped"),
                (false, Dir::Up) => format!("↑ {name} ok"),
                (false, Dir::Down) => format!("↓ {name} ok"),
            };
            println!(
                "{}",
                if *skipped {
                    paint.yellow(&text)
                } else {
                    paint.green(&text)
                }
            );
        }
        Event::Retrying {
            name,
            attempt,
            reason,
        } => {
            if !quiet {
                println!(
                    "{}",
                    paint.yellow(&format!("↻ {name}: retry {attempt} ({reason})"))
                );
            }
        }
        Event::Removing { name } => {
            if verbose && !quiet {
                println!("{} {name}", paint.dim("×"));
            }
        }
        Event::Removed { name } => {
            if quiet {
                return;
            }
            println!("{}", paint.yellow(&format!("× {name} removed")));
        }
        Event::Missing { name } => {
            if quiet {
                return;
            }
            println!("{}", paint.yellow(&format!("○ {name} missing")));
        }
        Event::Summary(s) => {
            let text = if s.removed > 0 {
                format!("removed {}, skipped {}", s.removed, s.skipped)
            } else {
                format!(
                    "uploaded {}, downloaded {}, skipped {}",
                    s.uploaded, s.downloaded, s.skipped
                )
            };
            println!(
                "{}",
                if s.failed.is_empty() {
                    paint.green(&text)
                } else {
                    paint.red(&text)
                }
            );
            if !s.failed.is_empty() {
                println!("{}", paint.red(&format!("failed: {}", s.failed.join(", "))));
            }
        }
    }
}
