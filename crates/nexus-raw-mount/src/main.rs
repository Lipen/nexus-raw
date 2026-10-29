//! `nxr-mount`: a FUSE view of a Nexus raw repository (Linux, read-only).
//!
//! The repository is a flat namespace of paths: a directory exists while any object sits under it.
//! This binary turns that namespace into a local tree.
//! Listings go through the search API, reads go through ranged GETs, and nothing is written.

#[cfg(target_os = "linux")]
mod fs;

#[cfg(target_os = "linux")]
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use clap::Parser;
#[cfg(target_os = "linux")]
use nexus_raw_core::{creds, Config, Nxr};

/// Mount a Nexus raw repository as a local directory.
#[cfg(target_os = "linux")]
#[derive(Debug, Parser)]
#[command(
    name = "nxr-mount",
    version,
    about = "Mount a Nexus raw repository as a local directory (read-only: listings and ranged reads)",
    after_help = "The mount is read-only: writes answer EROFS.\n\
                  Directory listings and file sizes come from the repository, bytes are fetched lazily."
)]
struct Cli {
    /// The repository (directory) URL to mount.
    url: String,
    /// The local directory to mount on.
    mountpoint: std::path::PathBuf,
    /// Credentials as user:pass, curl style.
    /// Env stays preferred for CI: NXR_AUTH (base64 user:pass) or NXR_USERNAME + NXR_PASSWORD.
    #[arg(short = 'u', long)]
    user: Option<String>,
    /// Skip TLS certificate verification.
    #[arg(long)]
    tls_insecure: bool,
    /// A PEM bundle of extra trust roots for a private CA (also NXR_CA_BUNDLE).
    /// Conflicts with --tls-insecure.
    #[arg(
        long,
        value_name = "FILE",
        env = "NXR_CA_BUNDLE",
        hide_env_values = true,
        conflicts_with = "tls_insecure"
    )]
    ca_bundle: Option<std::path::PathBuf>,
    /// Parallel requests and FUSE dispatch threads.
    #[arg(long, default_value_t = 8)]
    workers: usize,
    /// Attempts per HTTP request.
    #[arg(long, default_value_t = 4)]
    retry: u32,
    /// TCP connect timeout in seconds.
    #[arg(long, default_value_t = 15)]
    connect_timeout_secs: u64,
    /// Fail a transfer when no bytes arrive for this many seconds.
    #[arg(long, default_value_t = 30)]
    stall_secs: u64,
    /// Size of one read block, in KiB (minimum 4).
    #[arg(long, default_value_t = 256)]
    block_kib: u64,
    /// Read-block cache budget, in MiB (minimum 1).
    #[arg(long, default_value_t = 256)]
    cache_mib: u64,
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("nxr-mount: {message}");
            ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("nxr-mount: this build has no FUSE backend; the mount is Linux-only");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn run(cli: Cli) -> Result<(), String> {
    let explicit = match cli.user.as_deref() {
        Some(value) => {
            let (user, pass) = value
                .split_once(':')
                .ok_or_else(|| "-u expects user:pass (a single ':')".to_owned())?;
            Some((user, pass))
        }
        None => None,
    };
    let auth = creds::resolve(explicit)
        .map_err(|e| e.to_string())?
        .map(|c| c.header);
    let cfg = Config {
        base: cli.url.clone(),
        tls_insecure: cli.tls_insecure,
        ca_bundle: cli.ca_bundle,
        workers: cli.workers,
        retry_attempts: cli.retry,
        connect_timeout: Duration::from_secs(cli.connect_timeout_secs),
        stall_timeout: Duration::from_secs(cli.stall_secs),
        auth,
    };
    cfg.validate().map_err(|e| e.to_string())?;
    // A directory listing needs the repository name from the URL: a URL outside
    // `/repository/<name>/...` can never list anything, so refuse it here
    // instead of mounting a tree that stays empty.
    nexus_raw_core::layout::ls::repository_of(&cli.url).map_err(|e| e.to_string())?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    // The mount renders nothing: drain the event channel so senders never block.
    runtime.spawn(async move { while receiver.recv().await.is_some() {} });
    let nxr = Nxr::new(cfg, sender).map_err(|e| e.to_string())?;
    let base = nxr.base().trim_end_matches('/').to_owned();

    let filesystem = fs::NexusFs::new(
        Arc::new(nxr),
        runtime.handle().clone(),
        base,
        cli.block_kib.saturating_mul(1024).max(4096),
        usize::try_from(cli.cache_mib.saturating_mul(1024 * 1024))
            .unwrap_or(usize::MAX)
            .max(1024 * 1024),
    );

    let mut options = fuser::Config::default();
    options.mount_options = vec![
        fuser::MountOption::RO,
        fuser::MountOption::FSName(cli.url.clone()),
        fuser::MountOption::Subtype("nxr".to_owned()),
        fuser::MountOption::DefaultPermissions,
    ];
    options.acl = fuser::SessionACL::Owner;
    options.n_threads = Some(cli.workers.clamp(1, 16));
    options.clone_fd = true;

    fuser::mount(filesystem, &cli.mountpoint, &options)
        .map_err(|e| format!("mount {}: {e}", cli.mountpoint.display()))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn help_names_no_disk_cache() {
        let err = Cli::try_parse_from(["nxr-mount", "--help"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp);
        assert!(!err.to_string().contains("cache-dir"));
    }
}
