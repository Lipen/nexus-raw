//! Config: TOML profiles (URLs only), XDG path; merging with flags stays in the CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::Error;

/// The full core configuration; the CLI builds it from profile, flags and env.
#[derive(Debug, Clone)]
pub struct Config {
    pub base: String,
    pub workers: usize,
    pub retry_attempts: u32,
    pub connect_timeout: Duration,
    pub stall_timeout: Duration,
    pub tls_insecure: bool,
    pub auth: Option<String>,
}

impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        normalize_base(&self.base)?;
        if !(1..=64).contains(&self.workers) {
            return Err(Error::misuse(format!(
                "workers {} outside 1..=64",
                self.workers
            )));
        }
        if self.retry_attempts == 0 || self.retry_attempts > 32 {
            return Err(Error::misuse(format!(
                "retry attempts {} outside 1..=32",
                self.retry_attempts
            )));
        }
        if self.connect_timeout.is_zero() || self.stall_timeout.is_zero() {
            return Err(Error::misuse("timeouts must be positive"));
        }
        Ok(())
    }

    /// Alias used by the transport; keeps `normalize_base` import-free there.
    pub fn normalized_base(base: &str) -> Result<String, Error> {
        normalize_base(base)
    }
}

/// A profile from config.toml: URL and an optional TLS off-switch only.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub url: String,
    pub tls_insecure: Option<bool>,
}

/// The parsed config.toml.
#[derive(Debug, Clone, Default)]
pub struct ConfigFile {
    pub default_profile: Option<String>,
    pub profiles: BTreeMap<String, Profile>,
}

/// The config path: `explicit` → `$NXR_CONFIG` → XDG.
/// An explicit path or `$NXR_CONFIG` must exist; the default one may be absent.
pub fn config_path(explicit: Option<PathBuf>, no_config: bool) -> Result<Option<PathBuf>, Error> {
    if no_config {
        return Ok(None);
    }
    if let Some(p) = explicit {
        if !p.is_file() {
            return Err(Error::misuse(format!("config not found: {}", p.display())));
        }
        return Ok(Some(p));
    }
    if let Ok(p) = std::env::var("NXR_CONFIG") {
        let p = PathBuf::from(p);
        if !p.is_file() {
            return Err(Error::misuse(format!("config not found: {}", p.display())));
        }
        return Ok(Some(p));
    }
    Ok(xdg_path())
}

fn xdg_path() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
    };
    let p = base.join("nxr").join("config.toml");
    p.is_file().then_some(p)
}

/// Load and sane-check the config: auth/password keys in TOML are misuse.
pub fn load(path: &Path) -> Result<ConfigFile, Error> {
    let raw = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    let value: toml::Value =
        toml::from_str(&raw).map_err(|e| Error::misuse(format!("{}: {e}", path.display())))?;
    let table = value
        .as_table()
        .ok_or_else(|| Error::misuse(format!("{}: expected a table", path.display())))?;
    forbid_secrets(path, table, String::new())?;
    let mut cfg = ConfigFile::default();
    for (key, val) in table {
        match key.as_str() {
            "default_profile" => {
                cfg.default_profile = Some(
                    val.as_str()
                        .ok_or_else(|| {
                            Error::misuse(format!("{path:?}: default_profile must be a string"))
                        })?
                        .to_owned(),
                );
            }
            name => {
                let profile: Profile = val
                    .clone()
                    .try_into()
                    .map_err(|e| Error::misuse(format!("{path:?}: profile {name:?}: {e}")))?;
                cfg.profiles.insert(name.to_owned(), profile);
            }
        }
    }
    Ok(cfg)
}

fn forbid_secrets(
    path: &Path,
    table: &toml::map::Map<String, toml::Value>,
    prefix: String,
) -> Result<(), Error> {
    for (key, val) in table {
        let full = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if key == "auth" || key == "password" {
            return Err(Error::misuse(format!(
                "{:?}: key {full:?} is forbidden: passwords never live in the config",
                path.display()
            )));
        }
        if let Some(nested) = val.as_table() {
            forbid_secrets(path, nested, full)?;
        }
    }
    Ok(())
}

/// Base URL normalization: http/https scheme, a host, trailing `/`, no query/fragment.
pub fn normalize_base(base: &str) -> Result<String, Error> {
    let err = || {
        Error::misuse(format!(
            "invalid base URL {base:?}: need absolute http(s) URL"
        ))
    };
    let url = reqwest::Url::parse(base).map_err(|_| err())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(err());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::misuse(format!(
            "invalid base URL {base:?}: query and fragment are not allowed"
        )));
    }
    let mut s = url.as_str().to_owned();
    if !s.ends_with('/') {
        s.push('/');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_normalization() {
        assert_eq!(
            normalize_base("https://host/repository/raw/").unwrap(),
            "https://host/repository/raw/"
        );
        assert_eq!(
            normalize_base("https://host/repository/raw").unwrap(),
            "https://host/repository/raw/"
        );
        assert!(normalize_base("ftp://host/raw/").is_err());
        assert!(normalize_base("https://host/raw/?x=1").is_err());
        assert!(normalize_base("not a url").is_err());
    }

    #[test]
    fn config_rejects_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[dev]\nurl = \"http://x/\"\npassword = \"oops\"\n").unwrap();
        let err = load(&p).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("forbidden"));
    }

    #[test]
    fn config_rejects_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(&p, "[dev]\nurl = \"http://x/\"\nretrys = 3\n").unwrap();
        assert!(load(&p).is_err());
    }

    #[test]
    fn config_parses_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "default_profile = \"a\"\n[a]\nurl = \"http://a/\"\n[b]\nurl = \"http://b/\"\ntls_insecure = true\n",
        )
        .unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("a"));
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(cfg.profiles["b"].tls_insecure, Some(true));
    }
}
