//! `nxr` — thin CLI over the `nexus-raw-core` facade.
//!
//! This crate owns only flag parsing, event rendering and exit codes.
//! All protocol logic lives in the core crate.

mod cmd;
mod config_setup;
mod render;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Publish and fetch artifacts from a Nexus raw repository.
#[derive(Debug, Parser)]
#[command(name = "nxr", version, about)]
pub(crate) struct Cli {
    /// Profile name from the config file; selects the url and credentials.
    #[arg(long, global = true, value_name = "NAME")]
    pub(crate) profile: Option<String>,
    /// Base URL of the raw repository; wins over the profile url.
    #[arg(long, global = true, value_name = "URL")]
    pub(crate) base: Option<String>,
    /// Parallel artifact transfers.
    #[arg(long, global = true, value_name = "N", default_value_t = 8)]
    pub(crate) workers: usize,
    /// Attempts per HTTP request.
    #[arg(long, global = true, value_name = "N", default_value_t = 4)]
    pub(crate) retry: u32,
    /// TCP connect timeout in seconds.
    #[arg(long, global = true, value_name = "SECS", default_value_t = 15)]
    pub(crate) connect_timeout_secs: u64,
    /// Fail a transfer when no bytes arrive for this many seconds.
    #[arg(long, global = true, value_name = "SECS", default_value_t = 30)]
    pub(crate) stall_secs: u64,
    /// Skip TLS certificate verification.
    #[arg(long, global = true)]
    pub(crate) tls_insecure: bool,
    /// Path to config.toml (default: $NXR_CONFIG, then ~/.config/nxr/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) config: Option<PathBuf>,
    /// Ignore the config file entirely.
    #[arg(long, global = true)]
    pub(crate) no_config: bool,
    /// Emit one NDJSON event per line on stdout.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Print only the final summary line.
    #[arg(short = 'q', long, global = true)]
    pub(crate) quiet: bool,
    /// Print more detail: transfer starts and plan names.
    #[arg(short = 'v', long, global = true)]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Cmd,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Cmd {
    /// Publish a version from a local directory.
    Up {
        /// Directory holding claim.json, artifacts and .sha256 markers.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Claim file to use instead of <dir>/claim.json.
        #[arg(long, value_name = "FILE")]
        names: Option<PathBuf>,
        /// Print the plan without transferring anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Download a version, or a subset of it, into a directory.
    Down {
        /// Target directory; created when missing.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Version to download.
        #[arg(long, value_name = "V")]
        version: Option<String>,
        /// Pointer to resolve: latest or nightly.
        #[arg(long, value_name = "POINTER")]
        pointer: Option<String>,
        /// Restrict the transfer to these artifact names.
        #[arg(long, value_name = "NAME")]
        only: Vec<String>,
        /// Claim file whose artifacts join the --only filter.
        #[arg(long, value_name = "FILE")]
        names: Option<PathBuf>,
    },
    /// Check local bytes, markers and digests; no network.
    Verify {
        /// Directory holding claim.json, artifacts and .sha256 markers.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Claim file to use instead of <dir>/claim.json.
        #[arg(long, value_name = "FILE")]
        names: Option<PathBuf>,
    },
    /// Print the symmetric plan against the server; nothing is written.
    Diff {
        /// Directory holding claim.json, artifacts and .sha256 markers.
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Claim file to use instead of <dir>/claim.json.
        #[arg(long, value_name = "FILE")]
        names: Option<PathBuf>,
    },
    /// Per-name remote states of a version, or the version list.
    Ls {
        /// Version to inspect; without it the version list is printed.
        #[arg(long, value_name = "V")]
        version: Option<String>,
    },
    /// Atomically move a pointer (latest, nightly) to a version.
    Point {
        /// Pointer name: latest or nightly.
        #[arg(value_name = "POINTER")]
        pointer: String,
        /// Version the pointer should name.
        #[arg(value_name = "V")]
        version: String,
        /// Move the pointer only forward in version order.
        #[arg(long)]
        if_newer: bool,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    match runtime.block_on(cmd::dispatch(&cli)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::from(e.exit_code())
        }
    }
}
