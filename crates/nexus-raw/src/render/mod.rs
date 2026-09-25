//! Event rendering: human console output and the renderer task plumbing.

mod json;

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
/// The sender is cloned into the core; dropping the session's own sender after
/// the operation closes the channel and lets the renderer finish.
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
    while let Some(ev) = rx.recv().await {
        match mode {
            Mode::Human { quiet, verbose } => human(&ev, quiet, verbose),
            Mode::Json => json::line(&ev),
        }
    }
}

/// Human console rendering.
///
/// `-q` keeps only the final summary; `-v` adds transfer starts and plan names.
/// Coalesced byte-progress events are not printed in human mode.
fn human(ev: &Event, quiet: bool, verbose: bool) {
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
                    (*total)
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "?".into())
                );
            }
        }
        Event::ArtifactBytes { .. } => {}
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
        Event::Summary(s) => {
            println!(
                "uploaded {}, downloaded {}, skipped {}",
                s.uploaded, s.downloaded, s.skipped
            );
            if !s.failed.is_empty() {
                println!("failed: {}", s.failed.join(", "));
            }
        }
    }
}
