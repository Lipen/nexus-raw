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
            .with_context(|| format!("server {}", server.name))?;
        tabs.push(Tab::new(server.clone(), repos));
    }
    open_terminal(servers, tabs, cfg, cfg_path, args, auth).await
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
    let mut app = App::new(
        servers,
        tabs,
        cfg,
        cfg_path,
        args.all_formats,
        download_dir,
        auth,
        tx,
    );
    app.boot();

    // A panic mid-frame must not leave the terminal in raw mode.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
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
            if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
                match crossterm::event::read() {
                    Ok(ev) => {
                        if input_tx.blocking_send(ev).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    });

    enable_raw_mode().context("enable raw mode")?;
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )
    .context("enter alternate screen")?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(stdout())).context("terminal backend")?;
    let result = pump(&mut terminal, &mut app, &mut rx, &mut input_rx).await;

    let _ = disable_raw_mode();
    let _ = execute!(
        stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    );
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    result
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
                // Unix reports only presses; Windows also reports releases and repeats.
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
        }
    }
}
