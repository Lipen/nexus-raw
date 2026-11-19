//! Event rendering: human console output and the renderer task plumbing.

mod json;

use std::io::{IsTerminal, Write};

use nexus_raw_core::{Dir, Event};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Write one line of console output.
///
/// A reader that went away (`nxr --json up ... | head -1`) ends the run the way
/// a POSIX tool does when SIGPIPE fires: quietly, with 141.
/// Any other write failure is a real one, a full disk included, and says so.
pub(crate) fn out_fmt(args: std::fmt::Arguments<'_>) {
    let mut stdout = std::io::stdout().lock();
    let written = stdout
        .write_fmt(args)
        .and_then(|()| stdout.write_all(b"\n"));
    if let Err(err) = written {
        if err.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        eprintln!("nxr: cannot write to stdout: {err}");
        std::process::exit(1);
    }
}

/// `println!` for the console streams: same shape, no panic on a closed pipe.
macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::render::out_fmt(format_args!($($arg)*))
    };
}
pub(crate) use outln;

/// How the event stream is rendered.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Mode {
    Human { quiet: bool, verbose: bool },
    Json { stderr: bool },
}

impl Mode {
    pub(crate) fn from_flags(json: bool, json_stderr: bool, quiet: bool, verbose: bool) -> Self {
        if json {
            Self::Json {
                stderr: json_stderr,
            }
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
    let calm = calm_view(
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
    );
    while let Some(ev) = rx.recv().await {
        match mode {
            Mode::Human { quiet, verbose } => {
                human(&ev, quiet, verbose, calm, &mut progress, &paint);
            }
            Mode::Json { stderr } => json::line(&ev, stderr),
        }
    }
    progress.clear();
}

/// Whether the terminal gets the calm view: one progress line and a summary, no line per artifact.
///
/// Both streams must be terminals.
/// The progress line is drawn on stderr, so a redirected stderr would leave the transfer silent, and a redirected stdout is a log that wants every name.
fn calm_view(stdout_tty: bool, stderr_tty: bool) -> bool {
    stdout_tty && stderr_tty
}

/// Whether an artifact's completion earns its own line.
///
/// `-v` asks for the names back, and a piped or redirected stdout keeps them by default.
fn artifact_lines(verbose: bool, calm: bool) -> bool {
    verbose || !calm
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
pub(crate) fn human_bytes(n: u64) -> String {
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

/// The freshest byte event: the artifact and, for a range, its span.
#[derive(Clone)]
struct Current {
    name: String,
    dir: Dir,
    done: u64,
    total: Option<u64>,
    span: Option<(u64, Option<u64>)>,
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
    /// The artifact of the most recent byte event.
    current: Option<Current>,
    /// The previous rate sample: time and absolute bytes.
    last: Option<(std::time::Instant, u64)>,
    /// Smoothed bytes per second.
    rate: f64,
    /// Visible width of the last drawn line, for erasing it.
    width: u16,
}

impl Progress {
    fn note_bytes(&mut self, name: &str, dir: Dir, done: u64, total: Option<u64>) {
        self.current = Some(Current {
            name: name.to_owned(),
            dir,
            done,
            total,
            span: None,
        });
    }

    fn note_span_bytes(
        &mut self,
        name: &str,
        dir: Dir,
        done: u64,
        total: Option<u64>,
        start: u64,
        end: Option<u64>,
    ) {
        self.current = Some(Current {
            name: name.to_owned(),
            dir,
            done,
            total,
            span: Some((start, end)),
        });
    }

    /// A new artifact starts its own rate window.
    fn reset_rate(&mut self) {
        self.last = None;
        self.rate = 0.0;
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
        let Some(current) = self.current.clone() else {
            return;
        };
        let now = std::time::Instant::now();
        if let Some((last_at, last_done)) = self.last {
            let dt = now.duration_since(last_at).as_secs_f64();
            if dt >= 0.2 && current.done >= last_done {
                let instant = (current.done - last_done) as f64 / dt;
                self.rate = if self.rate == 0.0 {
                    instant
                } else {
                    self.rate * 0.7 + instant * 0.3
                };
                self.last = Some((now, current.done));
            }
        } else {
            self.last = Some((now, current.done));
        }
        let arrow = match current.dir {
            Dir::Up => "↑",
            Dir::Down => "↓",
        };
        let mut line = if self.files_total > 0 {
            format!("{arrow} {}/{} files", self.files_done, self.files_total)
        } else {
            arrow.to_owned()
        };
        line.push_str(" · ");
        line.push_str(&current.name);
        let (span_done, span_total) = if let Some((start, end)) = current.span {
            let total = end.map_or_else(
                || current.total.map(|t| t.saturating_sub(start)),
                |e| Some(e.saturating_sub(start).saturating_add(1)),
            );
            (current.done.saturating_sub(start), total)
        } else {
            (current.done, current.total)
        };
        match span_total.filter(|t| *t > 0) {
            Some(t) => {
                line.push_str(&format!(
                    " {}/{} ({}%)",
                    human_bytes(span_done),
                    human_bytes(t),
                    span_done * 100 / t
                ));
                if current.span.is_some() {
                    if let Some(object_total) = current.total {
                        line.push_str(&format!(
                            " · object {}/{}",
                            human_bytes(current.done),
                            human_bytes(object_total)
                        ));
                    }
                }
            }
            None => line.push_str(&format!(" {}", human_bytes(span_done))),
        }
        if self.rate > 0.0 {
            line.push_str(&format!(" · {}/s", human_bytes(self.rate as u64)));
            if let Some(t) = span_total.filter(|t| *t > 0) {
                let remaining = t.saturating_sub(span_done) as f64;
                line.push_str(&format!(" · ETA {}", human_eta(remaining / self.rate)));
            }
        }
        let pad = (self.width as usize).saturating_sub(line.len());
        eprint!("\r{line}{}", " ".repeat(pad));
        self.width = line.len().min(u16::MAX as usize) as u16;
    }
}

/// A short duration for the progress line.
fn human_eta(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total >= 3600 {
        format!("{}h {:02}m", total / 3600, (total % 3600) / 60)
    } else if total >= 60 {
        format!("{}:{:02}", total / 60, total % 60)
    } else {
        format!("{total}s")
    }
}

/// Human console rendering.
///
/// `-q` keeps the summary line and the failure lines.
/// `-v` adds transfer starts, plan names and a line per artifact.
/// Live byte progress draws one in-place stderr line when stderr is a terminal, and on a full terminal that line replaces the per-artifact lines.
fn human(ev: &Event, quiet: bool, verbose: bool, calm: bool, prog: &mut Progress, paint: &Paint) {
    // Any non-progress line starts clean, so the in-place drawing never smears.
    if !matches!(
        ev,
        Event::ArtifactBytes { .. } | Event::ArtifactSpanBytes { .. }
    ) {
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
            outln!(
                "plan: {} to upload, {} to download, {} up to date",
                paint.bold(&upload.len().to_string()),
                paint.bold(&download.len().to_string()),
                paint.bold(&skip.len().to_string())
            );
            prog.files_total = upload.len() + download.len() + skip.len();
            if verbose {
                for name in upload {
                    outln!("{} {name}", paint.dim("↑"));
                }
                for name in download {
                    outln!("{} {name}", paint.dim("↓"));
                }
                for name in skip {
                    outln!("{} {name}", paint.dim("○"));
                }
            }
        }
        Event::ArtifactStarted {
            name, dir, total, ..
        } => {
            prog.reset_rate();
            if verbose && !quiet {
                let arrow = match dir {
                    Dir::Up => "↑",
                    Dir::Down => "↓",
                };
                let size = (*total).map_or_else(|| "?".to_owned(), human_bytes);
                outln!("{} {name} {}", paint.dim(arrow), paint.dim(&size));
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
        Event::ArtifactSpanBytes {
            name,
            dir,
            done,
            total,
            start,
            end,
        } => {
            if quiet {
                return;
            }
            prog.note_span_bytes(name, *dir, *done, *total, *start, *end);
            prog.draw();
        }
        Event::ArtifactDone {
            name, dir, skipped, ..
        } => {
            if quiet {
                return;
            }
            prog.files_done += 1;
            if !artifact_lines(verbose, calm) {
                return;
            }
            let text = match (skipped, dir) {
                (true, _) => format!("○ {name} skipped"),
                (false, Dir::Up) => format!("↑ {name} ok"),
                (false, Dir::Down) => format!("↓ {name} ok"),
            };
            outln!(
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
                outln!(
                    "{}",
                    paint.yellow(&format!("↻ {name}: retry {attempt} ({reason})"))
                );
            }
        }
        Event::Removing { name } => {
            if verbose && !quiet {
                outln!("{} {name}", paint.dim("×"));
            }
        }
        Event::Removed { name } => {
            if quiet {
                return;
            }
            outln!("{}", paint.yellow(&format!("× {name} removed")));
        }
        Event::Missing { name } => {
            if quiet {
                return;
            }
            outln!("{}", paint.yellow(&format!("○ {name} missing")));
        }
        // Warnings are not chatter: they name rows the run had to leave out,
        // so they go to stderr and survive `--quiet` like a failure does.
        Event::Warning { text } => eprintln!("warning: {text}"),
        Event::Summary(s) => {
            let mut text = if s.removed > 0 {
                format!("removed {}, skipped {}", s.removed, s.skipped)
            } else {
                format!(
                    "uploaded {}, downloaded {}, skipped {}",
                    s.uploaded, s.downloaded, s.skipped
                )
            };
            if s.sealed > 0 {
                text.push_str(&format!(", sealed {}", s.sealed));
            }
            outln!(
                "{}",
                if s.failed.is_empty() {
                    paint.green(&text)
                } else {
                    paint.red(&text)
                }
            );
            if !s.failed.is_empty() {
                outln!("{}", paint.red(&format!("failed: {}", s.failed.join(", "))));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{artifact_lines, calm_view, human_eta};

    #[test]
    fn the_eta_reads_as_a_short_duration() {
        assert_eq!(human_eta(0.0), "0s");
        assert_eq!(human_eta(42.4), "42s");
        assert_eq!(human_eta(90.0), "1:30");
        assert_eq!(human_eta(3661.0), "1h 01m");
    }

    #[test]
    fn the_calm_view_needs_both_streams() {
        assert!(calm_view(true, true));
        assert!(
            !calm_view(true, false),
            "a redirected stderr has no progress line to show"
        );
        assert!(
            !calm_view(false, true),
            "a redirected stdout is a log that wants every name"
        );
        assert!(!calm_view(false, false));
    }

    #[test]
    fn a_pipe_keeps_every_name_and_a_terminal_keeps_the_verbose_ones() {
        assert!(artifact_lines(false, false));
        assert!(!artifact_lines(false, true));
        assert!(artifact_lines(true, true));
        assert!(artifact_lines(true, false));
    }
}
