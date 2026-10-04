//! `nxr-tui`: a terminal browser over a Nexus raw repository, built on the `nexus-raw-core` facade.
//!
//! Screens: the repositories of every configured server, then the tree of the
//! selected repository at full depth, with a subtree download action and a live
//! progress line.
//!
//! ```text
//! nxr-tui http://127.0.0.1:8099/
//! ```
//!
//! The bootstrap validates every base before the terminal switches to the
//! alternate screen; `--smoke` runs the same flow headless.

mod app;
mod args;
mod net;
mod smoke;
mod tui;
mod ui;

use std::process::ExitCode;
use std::sync::Arc;

use nexus_raw_core::Error;

use crate::args::Args;

fn main() -> ExitCode {
    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(Some(args)) => Arc::new(args),
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = if args.smoke {
        runtime.block_on(smoke::run(&args))
    } else {
        runtime.block_on(tui::run(args))
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let core = e.chain().find_map(|cause| cause.downcast_ref::<Error>());
            let code = core.map_or(1, Error::exit_code);
            eprintln!("error: {e:#}");
            if let Some(hint) = core.and_then(Error::hint) {
                eprintln!("hint: {hint}");
            }
            ExitCode::from(code)
        }
    }
}
