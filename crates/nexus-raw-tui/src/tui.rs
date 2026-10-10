//! The terminal loop: the fail-fast bootstrap, raw mode, the input thread and
//! the message pump.
//!
//! The bootstrap validates everything before the terminal changes mode: every
//! server must answer `service_repos` and every tab opens already populated.
//! Any failure returns before raw mode, so the caller prints `error:` + `hint:`
//! and exits with the core exit code while the terminal stays untouched.

use std::io::stdout;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::{App, Msg, Tab};
use crate::args::Args;
use crate::config::{ConfigFile, ServerCfg};
use crate::net;
use crate::ui;

/// Runs the TUI until the user quits.
///
/// # Errors
///
/// Returns the bootstrap error (fail-fast, before raw mode) and errors from
/// terminal setup or drawing.
pub async fn run(
    args: Arc<Args>,
    cfg: ConfigFile,
    cfg_path: Option<PathBuf>,
) -> anyhow::Result<()> {
    let auth = net::auth_header(&args.user).context("resolve credentials")?;
    let servers = args.resolve_servers(&cfg)?;
    // Fail-fast: every configured server must answer before the screen opens.
    let mut tabs = Vec::with_capacity(servers.len());
    for server in &servers {
        let repos = net::server_repos(server, auth.clone())
            .await
            .with_context(|| format!("server {}", server.name.escape_debug()))?;
        tabs.push(Tab::new(server.clone(), repos));
    }
    open_terminal(servers, tabs, cfg, cfg_path, args, auth).await
}

/// The effective format filter: the flag lifts it, the config default fills it in.
#[must_use]
pub fn all_formats_of(args: &Args, cfg: &ConfigFile) -> bool {
    args.all_formats || cfg.tui.all_formats
}

/// The terminal session: raw mode, the input thread, the pump and the restore.
async fn open_terminal(
    servers: Vec<ServerCfg>,
    tabs: Vec<Tab>,
    cfg: ConfigFile,
    cfg_path: Option<PathBuf>,
    args: Arc<Args>,
    auth: Option<String>,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    let download_dir = cfg.download.dir.clone();
    let all_formats = all_formats_of(&args, &cfg);
    let mut app = App::new(
        servers,
        tabs,
        cfg,
        cfg_path,
        all_formats,
        download_dir,
        auth,
        tx,
    );
    app.boot();

    // A panic mid-frame must not leave the terminal in raw mode.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));

    // The input thread is sync code: a bounded channel gives it `blocking_send`.
    let (input_tx, mut input_rx) = mpsc::channel::<Event>(256);
    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = Arc::clone(&stop);
    let input = std::thread::spawn(move || {
        loop {
            if input_stop.load(Ordering::Relaxed) {
                break;
            }
            // A bounded poll keeps the thread joinable after quit: a bare `read` would sleep forever.
            // A poll error means the input source is gone: break, like a read error.
            match crossterm::event::poll(Duration::from_millis(100)) {
                Ok(true) => match crossterm::event::read() {
                    Ok(ev) => {
                        if input_tx.blocking_send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => continue,
                Err(_) => break,
            }
        }
    });

    // Setup failures route through the same restore as normal exits: a tty
    // left in raw mode or in the alternate screen stays broken until `reset`.
    let setup = (|| -> anyhow::Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
        enable_raw_mode().context("enable raw mode")?;
        execute!(
            stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )
        .context("enter alternate screen")?;
        Terminal::new(CrosstermBackend::new(stdout())).context("terminal backend")
    })();
    let mut terminal = match setup {
        Ok(terminal) => terminal,
        Err(e) => {
            restore_terminal();
            stop.store(true, Ordering::Relaxed);
            let _ = input.join();
            return Err(e);
        }
    };
    let result = pump(&mut terminal, &mut app, &mut rx, &mut input_rx).await;

    restore_terminal();
    // The queue owner goes first: a full channel must not block the join.
    drop(input_rx);
    drop(rx);
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    result
}

/// Returns the terminal to the shell: raw mode off, main screen, no capture,
/// no paste bracket, visible cursor. Every exit path runs through this.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );
    // ratatui hides the cursor on every frame. Leaving the alternate screen
    // does not bring it back.
    let _ = execute!(stdout(), crossterm::cursor::Show);
}

async fn pump(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    input_rx: &mut mpsc::Receiver<Event>,
) -> anyhow::Result<()> {
    loop {
        terminal
            .draw(|frame| ui::draw(frame, app))
            .context("draw frame")?;
        if app.quit {
            return Ok(());
        }
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => app.handle(msg),
                None => return Ok(()),
            },
            ev = input_rx.recv() => match ev {
                // Unix reports only presses. Windows also reports releases and repeats.
                Some(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    app.handle(Msg::Key(key));
                }
                Some(Event::Key(_)) => {}
                Some(Event::Mouse(mouse)) => app.handle(Msg::Mouse(mouse)),
                Some(Event::Paste(text)) => app.handle(Msg::Paste(text)),
                Some(Event::Resize(_, _)) => app.handle(Msg::Redraw),
                Some(Event::FocusGained) | Some(Event::FocusLost) => {}
                // The input thread is gone: nothing else will ever arrive.
                None => return Ok(()),
            },
            // The 500 ms tick runs only while a toast lives or a card waits
            // for its metadata: idle time costs nothing.
            _ = tokio::time::sleep(Duration::from_millis(500)), if app.needs_tick() => {
                app.handle(Msg::Tick);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn the_flag_and_the_config_both_lift_the_format_filter() {
        let cfg = ConfigFile::default();
        let flag_only = Args::parse_from(["nxr-tui", "--all-formats", "http://a/"]);
        assert!(all_formats_of(&flag_only, &cfg));
        let plain = Args::parse_from(["nxr-tui", "http://a/"]);
        assert!(!all_formats_of(&plain, &cfg));
        let mut cfg_on = ConfigFile::default();
        cfg_on.tui.all_formats = true;
        assert!(all_formats_of(&plain, &cfg_on));
    }
}
