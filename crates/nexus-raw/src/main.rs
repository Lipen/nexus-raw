//! `nxr` is curl for a Nexus raw repository.
//!
//! This crate owns only flag parsing, event rendering and exit codes.
//! All protocol logic lives in the core crate.

mod cmd;
mod render;

use clap::{Parser, Subcommand};

use std::path::PathBuf;

/// curl for a Nexus raw repository: primitives with retries and TLS on, verified directory transfers, channel refs and manifests.
#[derive(Debug, Parser)]
#[command(name = "nxr", version, about)]
#[command(after_help = "\
Agent entry points:
  Every error carries a `hint:` line naming the next thing to check.
  `--json` emits NDJSON (exit 0 ok, 1 data, 2 misuse, 3 transport).
  Agent cookbook: https://lipen.github.io/nexus-raw/how-to/agents/
  Doc index:      https://lipen.github.io/nexus-raw/llms.txt
  Source:         https://github.com/Lipen/nexus-raw")]
pub(crate) struct Cli {
    /// Credentials as user:pass, curl style.
    /// Env stays preferred for CI: NXR_AUTH (base64 user:pass) or NXR_USERNAME + NXR_PASSWORD.
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
    /// Upload a local directory (§5.2).
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
        /// Upload this file first, alone, before any other name (claim-first).
        #[arg(long, value_name = "NAME")]
        claim_first: Option<String>,
        /// Skip marker generation and marker uploads.
        #[arg(long = "no-sha")]
        no_sha: bool,
        /// Print the plan without transferring anything.
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
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
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Ignore existing part files: every name downloads from zero.
        #[arg(long)]
        fresh: bool,
        /// Print the plan without transferring anything.
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Compare a local directory against a remote one: the delta report.
    Diff {
        /// The local directory.
        #[arg(value_name = "LOCAL_DIR")]
        local: PathBuf,
        /// The remote directory URL.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// Enumeration source: a manifest file, URL or `-` for stdin.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Delete the enumerated names from a remote directory (§5.4).
    Rm {
        /// The remote directory URL.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// Enumeration source: a manifest file, URL or `-` for stdin.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Print the plan without deleting anything.
        #[arg(long)]
        dry_run: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Delete a pointer file (§5.4).
    Point {
        /// Delete the pointer file the URL names.
        #[arg(long)]
        clear: bool,
        /// The pointer file URL.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Pour a version from one repository into another: bytes, markers, the version document.
    Mirror {
        /// The source directory URL: enumeration and bytes come from here.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// The destination directory URL.
        #[arg(value_name = "DST_URL")]
        dst: String,
        /// Enumeration source: a manifest file, URL or `-` for stdin.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Print the plan without transferring anything.
        #[arg(long)]
        dry_run: bool,
        /// Credentials for the source only, overriding the shared `-u`.
        #[arg(long, value_name = "USER:PASS")]
        src_user: Option<String>,
        /// Credentials for the destination only, overriding the shared `-u`.
        #[arg(long, value_name = "USER:PASS")]
        dst_user: Option<String>,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Move a version: mirror it into the destination, then delete the same names at the source.
    ///
    /// Nothing is deleted until the pour converged: a failed move leaves a duplicate, never a loss.
    Mv {
        /// The source directory URL.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// The destination directory URL.
        #[arg(value_name = "DST_URL")]
        dst: String,
        /// Enumeration source: a manifest file, URL or `-` for stdin.
        #[arg(long, value_name = "FILE|URL|-")]
        manifest: Option<String>,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Print both plans (the pour and the delete) without moving anything.
        #[arg(long)]
        dry_run: bool,
        /// Credentials for the source only, overriding the shared `-u`.
        #[arg(long, value_name = "USER:PASS")]
        src_user: Option<String>,
        /// Credentials for the destination only, overriding the shared `-u`.
        #[arg(long, value_name = "USER:PASS")]
        dst_user: Option<String>,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// List the entries of a raw directory URL, at any tree depth (--assets: flat artifact names).
    Ls {
        /// A directory URL inside a raw repository, the repository root included.
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
    /// Check local bytes, markers and digests (no network).
    Verify {
        /// The directory to check.
        #[arg(value_name = "DIR")]
        dir: PathBuf,
        /// Check exactly these names.
        #[arg(long, value_name = "FILE|-")]
        manifest: Option<String>,
    },
    /// Diagnose credentials, TLS and reachability.
    Doctor {
        /// A base URL to probe (checks without it stay local).
        #[arg(value_name = "URL")]
        url: Option<String>,
    },
    /// Server metadata through the Sonatype service REST API (not part of the storage protocol).
    Service {
        #[command(subcommand)]
        op: ServiceOp,
    },
    /// Print a shell completion script to stdout (bash, zsh, fish, powershell).
    Complete {
        /// The shell to generate the script for.
        #[arg(value_name = "SHELL")]
        shell: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ServiceOp {
    /// List the repositories of the server behind `url` (a server root or any repository URL).
    Repos {
        /// A server root URL or a URL anywhere inside the server.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// Server liveness and writability behind `url`.
    Status {
        /// A server root URL or a URL anywhere inside the server.
        #[arg(value_name = "URL")]
        url: String,
    },
    /// The repository behind `url`: what it is, and with --detail its full settings (nx-admin only).
    Repo {
        /// A URL inside the repository (the repository root, a version dir, a file).
        #[arg(value_name = "URL")]
        url: String,
        /// Also fetch the full repository settings: answers only to an nx-admin.
        #[arg(long)]
        detail: bool,
    },
    /// Every asset of the repository behind `url`, through the search API: any format.
    Assets {
        /// A repository URL (the repository root or anything inside it).
        #[arg(value_name = "URL")]
        url: String,
        /// The server-side search query, passed through.
        #[arg(long)]
        q: Option<String>,
        /// Keep only assets under this whole-segment path prefix (repeatable).
        #[arg(long)]
        prefix: Vec<String>,
    },
    /// The EULA gate of the server behind `url`: with --accept, open it (nx-admin).
    Eula {
        /// A server root URL or a URL anywhere inside the server.
        #[arg(value_name = "URL")]
        url: String,
        /// Accept the presented disclaimer when the gate is closed.
        #[arg(long)]
        accept: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ChannelOp {
    /// Print the current token.
    /// An unset channel prints `unset`.
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
        Ok(code) => std::process::ExitCode::from(code),
        Err(e) => {
            if cli.json {
                // The NDJSON channel must see the failure too: scripts parse this
                // line, they do not scrape stderr.
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "error",
                        "code": e.exit_code(),
                        "error": e.to_string(),
                        "hint": e.hint(),
                    })
                );
            }
            eprintln!("error: {e}");
            if let Some(hint) = e.hint() {
                eprintln!("hint: {hint}");
            }
            std::process::ExitCode::from(e.exit_code())
        }
    }
}
