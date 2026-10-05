//! Command line arguments for `nxr-tui`.

use clap::Parser;
use nexus_raw_core::Error;

use crate::config::{self, ConfigFile, ServerCfg};

/// A terminal browser over Nexus raw repositories.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "nxr-tui",
    version,
    after_help = after_help(),
)]
pub struct Args {
    /// Server root URLs, e.g. http://127.0.0.1:8081/
    /// With none given, every [[server]] preset of the config opens.
    pub bases: Vec<String>,

    /// Open the named server preset from the config file (repeatable)
    #[arg(short, long)]
    pub server: Vec<String>,

    /// Credentials as user:pass, curl style
    /// (env fallback: NXR_AUTH, or NXR_USERNAME + NXR_PASSWORD)
    #[arg(short, long)]
    pub user: Option<String>,

    /// Let repositories of every format be opened, not only raw
    #[arg(long)]
    pub all_formats: bool,

    /// Headless mode: list the tree, download a subtree, print the summary
    #[arg(long)]
    pub smoke: bool,

    /// Alternative config file path
    /// (default: $XDG_CONFIG_HOME/nxr-tui/config.toml)
    #[arg(long)]
    pub config: Option<std::path::PathBuf>,

    /// Write the commented template config to the config path and exit
    #[arg(long)]
    pub init_config: bool,
}

fn after_help() -> String {
    format!(
        "config: {} (nxr-tui --init-config writes a template)\n\
         keys:   up/down or k/j move, enter opens, d downloads, / filters, ? help, s servers, q quits",
        config::default_path()
            .map_or_else(|| "(no config directory: set XDG_CONFIG_HOME or HOME)".to_owned(), |p| p.display().to_string())
    )
}

impl Args {
    /// The servers this invocation opens: positional URLs first, then `--server`
    /// presets, then every config preset when neither was given.
    ///
    /// # Errors
    ///
    /// Returns an error when a preset name is unknown or the resolution ends
    /// with no server at all (a misuse: exit code 2).
    pub fn resolve_servers(&self, cfg: &ConfigFile) -> anyhow::Result<Vec<ServerCfg>> {
        let mut out: Vec<ServerCfg> = Vec::new();
        for base in &self.bases {
            let server = ServerCfg::from_base(base.clone());
            // The same URL twice is one tab, like every other open path.
            let key = server.url.trim_end_matches('/');
            if out.iter().any(|s| s.url.trim_end_matches('/') == key) {
                continue;
            }
            out.push(server);
        }
        for name in &self.server {
            let Some(preset) = cfg.servers.iter().find(|s| &s.name == name) else {
                let known = cfg
                    .servers
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Error::Misuse(format!(
                    "no server preset {name:?} in the config (known presets: {known})"
                ))
                .into());
            };
            out.push(preset.clone());
        }
        if out.is_empty() {
            out.extend(cfg.servers.iter().cloned());
        }
        if out.is_empty() {
            return Err(Error::Misuse(format!(
                "no servers: pass BASE_URL..., add a [[server]] preset to the config, \
                 or run `nxr-tui --init-config` for a template ({})",
                config::target_path(self.config.clone())
                    .map_or_else(|_| "see --config".to_owned(), |p| p.display().to_string())
            ))
            .into());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(bases: &[&str], servers: &[&str]) -> Args {
        Args::parse_from(
            std::iter::once("nxr-tui")
                .chain(bases.iter().copied())
                .chain(servers.iter().flat_map(|s| ["--server", s])),
        )
    }

    fn preset(name: &str, url: &str) -> ServerCfg {
        ServerCfg {
            name: name.to_owned(),
            url: url.to_owned(),
        }
    }

    #[test]
    fn positional_bases_become_host_named_servers() {
        let parsed = args(&["http://127.0.0.1:8081/"], &[]);
        let got = parsed.resolve_servers(&ConfigFile::default()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "127.0.0.1:8081");
        assert_eq!(got[0].url, "http://127.0.0.1:8081/");
    }

    #[test]
    fn the_same_url_twice_opens_one_tab() {
        let parsed = args(&["http://a/", "http://a/", "http://a/"], &[]);
        let got = parsed.resolve_servers(&ConfigFile::default()).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn a_named_preset_resolves_from_the_config() {
        let mut cfg = ConfigFile::default();
        cfg.servers.push(preset("main", "http://a/"));
        let parsed = args(&[], &["main"]);
        let got = parsed.resolve_servers(&cfg).unwrap();
        assert_eq!(got, vec![preset("main", "http://a/")]);
    }

    #[test]
    fn an_unknown_preset_names_the_known_ones() {
        let mut cfg = ConfigFile::default();
        cfg.servers.push(preset("main", "http://a/"));
        let err = args(&[], &["nope"]).resolve_servers(&cfg).unwrap_err();
        assert!(err.to_string().contains("main"), "{err}");
    }

    #[test]
    fn no_arguments_open_every_config_preset() {
        let mut cfg = ConfigFile::default();
        cfg.servers.push(preset("main", "http://a/"));
        cfg.servers.push(preset("backup", "http://b/"));
        let got = args(&[], &[]).resolve_servers(&cfg).unwrap();
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn no_servers_anywhere_is_misuse() {
        let err = args(&[], &[])
            .resolve_servers(&ConfigFile::default())
            .unwrap_err();
        let core = err.chain().find_map(|c| c.downcast_ref::<Error>());
        assert_eq!(core.map(Error::exit_code), Some(2));
        assert!(err.to_string().contains("no servers"), "{err}");
    }

    #[test]
    fn flags_parse() {
        let parsed = Args::parse_from([
            "nxr-tui",
            "--all-formats",
            "--smoke",
            "--config",
            "/tmp/x.toml",
            "-u",
            "u:p",
            "http://a/",
        ]);
        assert!(parsed.all_formats);
        assert!(parsed.smoke);
        assert_eq!(
            parsed.config.as_deref(),
            Some(std::path::Path::new("/tmp/x.toml"))
        );
        assert_eq!(parsed.user.as_deref(), Some("u:p"));
        assert_eq!(parsed.bases, vec!["http://a/"]);
    }

    #[test]
    fn init_config_is_a_flag() {
        let parsed = Args::parse_from(["nxr-tui", "--init-config"]);
        assert!(parsed.init_config);
    }
}
