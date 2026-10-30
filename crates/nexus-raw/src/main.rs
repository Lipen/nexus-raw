//! `nxr` is curl for a Nexus raw repository.
//!
//! This crate owns only flag parsing, event rendering and exit codes.
//! All protocol logic lives in the core crate.

mod alias;
mod cmd;
mod render;

use clap::{Parser, Subcommand};

use std::path::PathBuf;

/// curl for a Nexus raw repository: primitives with retries and TLS on, verified directory transfers and channel refs.
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
    #[arg(
        short = 'u',
        long,
        value_name = "USER:PASS",
        global = true,
        help_heading = "Global options"
    )]
    pub(crate) user: Option<String>,
    /// A remote alias from the config file (the file is read only when this names one).
    /// The alias expands URL arguments that look relative and supplies the credentials.
    /// A `-u` on the same call wins over the alias credentials.
    #[arg(
        short = 'R',
        long,
        value_name = "ALIAS",
        global = true,
        help_heading = "Global options"
    )]
    pub(crate) remote: Option<String>,
    /// The alias credentials resolved by `-R` (never printed).
    /// Kept apart from `user` so the doctor's source line stays honest.
    #[arg(skip)]
    pub(crate) alias_creds: Option<(String, String)>,
    /// The alias name behind `alias_creds`, for the doctor's source line.
    #[arg(skip)]
    pub(crate) alias_name: Option<String>,
    /// Parallel artifact transfers.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "N",
        default_value_t = 8
    )]
    pub(crate) workers: usize,
    /// Attempts per HTTP request.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "N",
        default_value_t = 4
    )]
    pub(crate) retry: u32,
    /// TCP connect timeout in seconds.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "SECS",
        default_value_t = 15
    )]
    pub(crate) connect_timeout_secs: u64,
    /// Fail a transfer when no bytes arrive for this many seconds.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "SECS",
        default_value_t = 30
    )]
    pub(crate) stall_secs: u64,
    /// Skip TLS certificate verification.
    #[arg(long, global = true, help_heading = "Global options")]
    pub(crate) tls_insecure: bool,
    /// A PEM bundle of extra trust roots for a private CA (curl's --cacert).
    /// The bundle carries the CA that signed the server certificate, not the bare server leaf.
    /// Conflicts with --tls-insecure: a bundle is the safe alternative to disabling verification.
    #[arg(
        long,
        global = true,
        help_heading = "Global options",
        value_name = "FILE",
        env = "NXR_CA_BUNDLE",
        hide_env_values = true,
        conflicts_with = "tls_insecure"
    )]
    pub(crate) ca_bundle: Option<PathBuf>,
    /// Emit one NDJSON event per line on stdout.
    #[arg(long, global = true, help_heading = "Global options")]
    pub(crate) json: bool,
    /// Print the summary line, plus any failures.
    #[arg(short = 'q', long, global = true, help_heading = "Global options")]
    pub(crate) quiet: bool,
    /// Print more detail: transfer starts and plan names.
    #[arg(short = 'v', long, global = true, help_heading = "Global options")]
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
        /// Must be a real path: `<out>.part` is created beside it while the transfer runs, and `--continue` resumes from that part.
        #[arg(short = 'o', long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Resume from an existing `<out>`.part through a Range request.
        #[arg(long = "continue")]
        cont: bool,
        /// Fetch through N concurrent byte-range requests (2 to 64).
        /// File output only: `--segments` with a stdout target is a misuse.
        #[arg(long, value_name = "N")]
        segments: Option<usize>,
        /// Fetch one byte range: START-END or START- (inclusive), through a Range request.
        /// Conflicts with --continue and --segments.
        #[arg(long, value_name = "START-END", conflicts_with_all = ["cont", "segments"])]
        range: Option<String>,
    },
    /// PUT a file, optionally with its sha-sibling marker.
    Put {
        /// The object URL.
        #[arg(value_name = "URL")]
        url: String,
        /// The file to send.
        #[arg(short = 'f', long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Read the bytes from stdin instead of a file: the body is buffered
        /// (Content-Length is part of the protocol), then sent as one PUT.
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
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
        /// Upload this file first, alone, before any other name (claim-first).
        #[arg(long, value_name = "NAME")]
        claim_first: Option<String>,
        /// Skip marker generation and marker uploads.
        #[arg(long = "no-sha")]
        no_sha: bool,
        /// Skip names whose remote marker digest already equals the local copy, re-upload the changed rest.
        #[arg(long)]
        if_changed: bool,
        /// After the run, poll the search index until every uploaded name is visible (the index lags behind writes on a real server).
        #[arg(long)]
        wait_index: bool,
        /// The visibility wait budget for --wait-index.
        #[arg(long, value_name = "SECS", default_value_t = 30)]
        index_timeout: u64,
        /// Print the plan without transferring anything.
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        /// The selected subtrees are scanned; entries outside them are never read.
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Download a remote directory, or one file named by its URL, into a local one (§5.2, §5.2.1).
    Down {
        /// The remote directory URL, or a file URL to fetch that one artifact.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// The target directory.
        #[arg(value_name = "DST_DIR")]
        dst: PathBuf,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Read names from a file, one per line (`-` is stdin).
        /// Repeatable, combines with --name.
        #[arg(long, value_name = "FILE")]
        names_from: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Ignore existing part files: every name downloads from zero.
        #[arg(long)]
        fresh: bool,
        /// Split each artifact into N concurrent byte-range requests (2 to 64).
        /// The worker budget still caps the total concurrency.
        #[arg(long, value_name = "N")]
        segments: Option<usize>,
        /// Print the plan without transferring anything.
        #[arg(long, visible_alias = "dry-run")]
        plan: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Clone a raw repository into a local directory: verified, resumable, with an optional local fact cache.
    Clone {
        /// The repository directory URL: the repository root or any prefix inside it.
        #[arg(value_name = "URL")]
        url: String,
        /// The target directory.
        #[arg(value_name = "DIR")]
        dst: PathBuf,
        /// Where to keep the local fact cache (default: DIR/.nxr-state.json).
        #[arg(long, value_name = "FILE", conflicts_with = "no_state")]
        state: Option<PathBuf>,
        /// Do not read or write the local fact cache: hash every local file.
        #[arg(long)]
        no_state: bool,
        /// Ignore the cache and hash every local file, even unchanged ones.
        #[arg(long)]
        verify: bool,
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
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Read names from a file, one per line (`-` is stdin).
        /// Repeatable, combines with --name.
        #[arg(long, value_name = "FILE")]
        names_from: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
        /// Build the remote side from the search API's per-asset facts (one paged listing) instead of per-name probes.
        /// Requires `--ls`.
        /// A server without the search API refuses unless `--fallback` degrades the report to the probe-based enumeration.
        #[arg(long, value_name = "SOURCE")]
        via: Option<String>,
        /// With `--via`: fall back to the probe-based delta when the search API refuses (400/404), instead of failing the run.
        #[arg(long, requires = "via")]
        fallback: bool,
    },
    /// Audit a repository for integrity: the structural report, optionally byte-verified.
    Audit {
        /// The repository directory URL to audit.
        #[arg(value_name = "REPO_URL")]
        repo: String,
        /// Download every asset and hash it against its marker: the only pass that catches silent bitrot.
        #[arg(long)]
        bytes: bool,
        /// Byte-check a deterministic sample of N assets instead of all (requires --bytes).
        #[arg(long, value_name = "N", requires = "bytes")]
        sample: Option<usize>,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Ceiling of the worker pool for the marker and byte fetches.
        #[arg(long, value_name = "N")]
        workers: Option<usize>,
        /// Emit NDJSON events instead of the human report.
        #[arg(long)]
        json: bool,
    },
    /// Delete the enumerated names from a remote directory (§5.4): a plan by default, deletion with --yes.
    Rm {
        /// The remote directory URL.
        #[arg(value_name = "SRC_URL")]
        src: String,
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Read names from a file, one per line (`-` is stdin).
        /// Repeatable, combines with --name.
        #[arg(long, value_name = "FILE")]
        names_from: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Delete for real.
        /// Without it the run prints the plan and touches nothing.
        #[arg(long)]
        yes: bool,
        /// Keep only names under this whole-segment prefix (repeatable).
        #[arg(long, value_name = "PREFIX")]
        prefix: Vec<String>,
    },
    /// Delete a pointer file (§5.4).
    Point {
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
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Read names from a file, one per line (`-` is stdin).
        /// Repeatable, combines with --name.
        #[arg(long, value_name = "FILE")]
        names_from: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Print the plan without transferring anything (`--plan` is the same flag).
        #[arg(long, visible_alias = "plan")]
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
        /// One explicit name (repeatable).
        #[arg(long, value_name = "NAME")]
        name: Vec<String>,
        /// Read names from a file, one per line (`-` is stdin).
        /// Repeatable, combines with --name.
        #[arg(long, value_name = "FILE")]
        names_from: Vec<String>,
        /// Best-effort enumeration through the server search API.
        #[arg(long)]
        ls: bool,
        /// Move for real.
        /// Without it the run prints both plans (the pour and the delete) and touches nothing.
        #[arg(long)]
        yes: bool,
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
        /// `--assets` requires a version directory: `/repository/<name>/<group…>/<version>/`.
        #[arg(value_name = "URL")]
        url: String,
        /// List the object names under a version directory URL.
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
    },
    /// Diagnose credentials, TLS and reachability.
    Doctor {
        /// A base URL to probe (checks without it stay local).
        #[arg(value_name = "URL")]
        url: Option<String>,
        /// Probe the server's behavior: range, content-length, search, repo type, version.
        /// Read-only.
        #[arg(long, requires = "url")]
        deep: bool,
        /// The object the deep probes target (default: a soft autolookup of the root).
        #[arg(long, value_name = "NAME", requires = "deep")]
        probe_name: Option<String>,
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
    // Windows gives the main thread 1 MB and the deep async frames of the
    // transfer paths overflow it: the run continues on a thread with an
    // explicit budget, so every platform gets the same generous stack.
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(run_main)
        .expect("the main thread spawns the work thread")
        .join()
        .expect("the work thread carries the exit code")
}

fn run_main() -> std::process::ExitCode {
    // The NDJSON channel must carry a parse failure too: clap would otherwise
    // print its own text and exit before the renderer exists, breaking the
    // `--json` contract the docs promise.
    let json = std::env::args_os().any(|arg| arg == "--json");
    let mut cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = u8::try_from(e.exit_code()).unwrap_or(2);
            if code == 0 {
                let _ = e.print();
                return std::process::ExitCode::SUCCESS;
            }
            let detail = e
                .to_string()
                .lines()
                .next()
                .unwrap_or("bad command line")
                .trim_start_matches("error: ")
                .to_owned();
            let hint = "check the command shape: nxr --help, or nxr <command> --help";
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "error",
                        "code": 2,
                        "error": detail,
                        "hint": hint,
                    })
                );
            } else {
                let _ = e.print();
                eprintln!("hint: {hint}");
            }
            return std::process::ExitCode::from(2);
        }
    };
    if let Err(e) = alias::apply_remote(&mut cli) {
        if cli.json {
            // The NDJSON channel carries the failure alone: scripts parse this
            // line, and a plaintext echo on stderr would leak into their logs.
            println!(
                "{}",
                serde_json::json!({
                    "event": "error",
                    "code": e.exit_code(),
                    "error": e.to_string(),
                    "hint": e.hint(),
                })
            );
        } else {
            eprintln!("error: {e}");
            if let Some(h) = e.hint() {
                eprintln!("hint: {h}");
            }
        }
        return std::process::ExitCode::from(e.exit_code());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    match runtime.block_on(cmd::dispatch(&cli)) {
        Ok(code) => std::process::ExitCode::from(code),
        Err(e) => {
            if cli.json {
                // The NDJSON channel carries the failure alone: scripts parse
                // this line, they do not scrape stderr.
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "error",
                        "code": e.exit_code(),
                        "error": e.to_string(),
                        "hint": e.hint(),
                    })
                );
            } else {
                eprintln!("error: {e}");
                if let Some(hint) = e.hint() {
                    eprintln!("hint: {hint}");
                }
            }
            std::process::ExitCode::from(e.exit_code())
        }
    }
}
