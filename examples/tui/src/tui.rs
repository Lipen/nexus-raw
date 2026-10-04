//! The terminal loop: raw mode, the input thread and the message pump.

use std::io::stdout;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as TermEvent};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use crate::app::{App, Msg};
use crate::args::Args;
use crate::ui;

/// Runs the TUI until the user quits.
///
/// # Errors
///
/// Returns an error when the terminal cannot be set up or drawing fails.
pub async fn run(args: Arc<Args>) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    // The input thread is sync code: a bounded channel gives it `blocking_send`.
    let (input_tx, mut input_rx) = mpsc::channel::<TermEvent>(256);
    enable_raw_mode().context("enable raw mode")?;
    execute!(stdout(), EnterAlternateScreen).context("enter alternate screen")?;
    // A panic mid-frame must not leave the terminal in raw mode.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        previous_hook(info);
    }));

    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = Arc::clone(&stop);
    let input = std::thread::spawn(move || {
        loop {
            if input_stop.load(Ordering::Relaxed) {
                break;
            }
            // A bounded poll keeps the thread joinable after quit: a bare `read` would sleep forever.
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                match event::read() {
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

    let mut app = App::new(args, tx.clone()).context("resolve credentials")?;
    app.boot();
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).context("terminal backend")?;
    let result = pump(&mut terminal, &mut app, &mut rx, &mut input_rx).await;

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    stop.store(true, Ordering::Relaxed);
    let _ = input.join();
    result
}

async fn pump(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<Msg>,
    input_rx: &mut mpsc::Receiver<TermEvent>,
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
                Some(TermEvent::Key(key)) => app.handle(Msg::Key(key)),
                Some(TermEvent::Resize(_, _)) => app.handle(Msg::Redraw),
                Some(_) => {}
                // The input thread is gone: nothing else will ever arrive.
                None => return Ok(()),
            },
        }
    }
}
