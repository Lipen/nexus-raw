//! The `nxr-tui` binary: parse, dispatch, exit codes.
//! Everything else lives in the library: `lib.rs` documents the layout.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use nexus_raw_core::Error;

use nxr_tui::{args::Args, config, smoke, tui};

fn main() -> ExitCode {
    let args = Arc::new(Args::parse());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let result = runtime.block_on(dispatch(&args));
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

async fn dispatch(args: &Arc<Args>) -> anyhow::Result<()> {
    // `--init-config` writes the template and exits, before anything else runs.
    if args.init_config {
        let path = config::target_path(args.config.clone())?;
        config::init(&path)?;
        println!("wrote {}", path.display());
        return Ok(());
    }
    let path = config::target_path(args.config.clone())?;
    let cfg = config::load(&path).context("load config")?;
    if args.smoke {
        smoke::run(args, cfg).await
    } else {
        tui::run(Arc::clone(args), cfg, Some(path)).await
    }
}
