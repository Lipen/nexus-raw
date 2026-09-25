//! Merges global flags, the TOML profile and env credentials into a core `Config`.

use std::time::Duration;

use nexus_raw_core::config as core_config;
use nexus_raw_core::{Config, Error, creds};

use crate::Cli;

/// Placeholder base for local-only commands (`verify`) that never touch the network.
const LOCAL_BASE: &str = "http://localhost/";

/// Build the core `Config` from CLI flags, the config file and env credentials.
///
/// `network` is false only for `verify`: it skips the profile/base requirement
/// and credential resolution, since nothing is sent anywhere.
pub(crate) fn build_config(cli: &Cli, network: bool) -> Result<Config, Error> {
    let path = core_config::config_path(cli.config.clone(), cli.no_config)?;
    let file = match &path {
        Some(p) => Some(core_config::load(p)?),
        None => None,
    };
    let profile_name = cli
        .profile
        .clone()
        .or_else(|| file.as_ref().and_then(|f| f.default_profile.clone()));
    let profile = profile_name
        .as_ref()
        .map(|name| {
            file.as_ref()
                .and_then(|f| f.profiles.get(name))
                .cloned()
                .ok_or_else(|| Error::Misuse(format!("profile {name:?} not found in config")))
        })
        .transpose()?;
    let base = match cli.base.clone().or_else(|| profile.as_ref().map(|p| p.url.clone())) {
        Some(b) => core_config::normalize_base(&b)?,
        None if network => {
            return Err(Error::Misuse(
                "no repository url: pass --base, --profile, or set default_profile in the config"
                    .into(),
            ));
        }
        None => LOCAL_BASE.into(),
    };
    let tls_insecure = cli.tls_insecure || profile.as_ref().is_some_and(|p| p.tls_insecure == Some(true));
    let auth = if network {
        creds::resolve(profile_name.as_deref())?.map(|c| c.header)
    } else {
        None
    };
    let cfg = Config {
        base,
        workers: cli.workers,
        retry_attempts: cli.retry,
        connect_timeout: Duration::from_secs(cli.connect_timeout_secs),
        stall_timeout: Duration::from_secs(cli.stall_secs),
        tls_insecure,
        auth,
    };
    cfg.validate()?;
    Ok(cfg)
}
