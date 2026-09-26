//! `nxr` is curl for a Nexus raw repository.
//!
//! This crate owns only flag parsing, event rendering and exit codes.
//! All protocol logic lives in the core crate.

mod cmd;
mod render;

use clap::{Parser, Subcommand};

use std::path::PathBuf;

/// curl for a Nexus raw repository: primitives with retries and TLS on,
/// verified directory transfers, channel refs and manifests.
#[derive(Debug, Parser)]
#[command(name = "nxr", version, about)]
pub(crate) struct Cli {
    /// Credentials as user:pass, curl style.
    /// Env stays preferred for CI: NXR_AUTH (base64 user:pass) or
    /// NXR_USERNAME + NXR_PASSWORD.
    #[arg(short = 'u', long, value_name = "USER:PASS", global = true)]
    pub(crate) user: Option<String>,
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
    /// GET a URL to a file or stdout.
    Get {
        /// The object URL.
        #[arg(value_name = "URL")]
        url: String,
        /// Output file (stdout when omitted).
        #[arg(short = 'o', long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Resume from an existing `<out>`.part through a Range request.
        #[arg(long = "continue")]
        cont: bool,
    },
    /// PUT a file, optionally with its sha-sibling marker.
    Put {
        /// The object URL.
        #[arg(value_name = "URL")]
        url: String,
        /// The file to send.
        #[arg(short = 'f', long, value_name = "FILE")]
        file: PathBuf,
        /// Also PUT `<url>`.sha256 with the sha256sum-style marker.
        #[arg(long)]
        sha: bool,
    },
    /// HEAD a URL: status, size, content type.
    Head {
        /// The object URL.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// The sha256 of a file or URL.
    Sha {
        /// A local path or an http(s) URL.
        #[arg(value_name = "FILE|URL")]
        target: String,
    },
    /// Upload a local directory: verified, parallel, marker-perfect (§5.2).
    Up {
        /// The source directory.
        #[arg(value_name = "SRC_DIR")]
        src: PathBuf,
        /// The remote directory URL.
        #[arg(value_name = "DST_URL")]
        dst: String,
        /// Restrict the transfer to these names.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// Skip marker generation and marker uploads.
        #[arg(long = "no-sha")]
        no_sha: bool,
        /// Print the plan without transferring anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Download a remote directory into a local one (§5.2, §5.2.1).
    Down {
        /// The remote directory URL.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// The target directory.
        #[arg(value_name = "DST_DIR")]
        dst: PathBuf,
        /// Enumeration source: a manifest file, URL or `-` for stdin.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// One explicit name; repeat as needed.
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Resume interrupted downloads from their part files.
        #[arg(long = "continue")]
        cont: bool,
    },
    /// List versions or the objects of a version directory (experimental).
    Ls {
        /// A repository/group URL (versions) or a directory URL (--assets).
        #[arg(value_name = "URL")]
        url: String,
        /// List the object names under a directory URL.
        #[arg(long)]
        assets: bool,
    },
    /// Read or write a channel ref: a token file with any name.
    Channel {
        #[command(subcommand)]
        op: ChannelOp,
    },
    /// Check local bytes, markers and digests. No network.
    Verify {
        /// The directory to check.
        #[arg(value_name = "DIR")]
        dir: PathBuf,
        /// Check exactly these names.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
    },
    /// Diagnose credentials, TLS and reachability.
    Doctor {
        /// A base URL to probe; checks without it stay local.
        #[arg(value_name = "URL")]
        url: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ChannelOp {
    /// Print the current token, or nothing when the channel is unset.
    Get {
        /// The channel file URL.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Write the token, optionally only forward in version order.
    Set {
        /// The channel file URL.
        #[arg(value_name = "URL")]
        url: String,
        /// The new token.
        #[arg(value_name = "TOKEN")]
        token: String,
        /// Keep the current token when it already compares >= the new one.
        #[arg(long)]
        if_forward: bool,
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
            if let Some(hint) = e.hint() {
                eprintln!("hint: {hint}");
            }
            std::process::ExitCode::from(e.exit_code())
        }
    }
}
