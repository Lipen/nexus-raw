//! The `nxr-tui` config file: server presets and defaults.
//!
//! The path is `$XDG_CONFIG_HOME/nxr-tui/config.toml`, falling back to
//! `$HOME/.config/nxr-tui/config.toml` (`--config` overrides both).
//! A missing file is the default config, so the TUI works with nothing on disk.
//! Passwords never live here: credentials come from `-u` or the environment at
//! every launch, exactly like the `nxr` CLI.

use std::path::{Path, PathBuf};

use nexus_raw_core::Error;
use serde::{Deserialize, Serialize};

/// The header every saved config carries: the comment block explains the rules.
/// `save` rewrites the file as plain TOML, so the header is the only comment
/// that survives a re-save.
pub const HEADER: &str = "\
# nxr-tui config: server presets and defaults.
# Passwords never live here: pass -u user:pass or export
# NXR_AUTH / NXR_USERNAME + NXR_PASSWORD at launch.";

/// The commented template `--init-config` writes.
pub const TEMPLATE: &str = "\
# nxr-tui config: server presets and defaults.
# Passwords never live here: pass -u user:pass or export
# NXR_AUTH / NXR_USERNAME + NXR_PASSWORD at launch.

version = 1

[download]
# Where downloads land, relative to the working directory of nxr-tui.
dir = \"nxr-tui-downloads\"

[tui]
# Let repositories of every format be opened, not only raw.
all_formats = false

# Every server is a preset: open it with `nxr-tui --server <name>`,
# or with no arguments at all. The `s` overlay inside the TUI adds one here.
[[server]]
name = \"main\"
url = \"http://127.0.0.1:8081/\"
";

/// One server preset: a label and a server root URL.
/// The label names the tab; the URL is what every request resolves against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCfg {
    /// The preset name and tab label.
    pub name: String,
    /// The server root URL, e.g. `http://127.0.0.1:8081/`.
    pub url: String,
}

impl ServerCfg {
    /// A server from a bare URL: the label is the host.
    /// This is how `nxr-tui http://host:port/` names its tab.
    #[must_use]
    pub fn from_base(url: String) -> Self {
        Self {
            name: host_of(&url),
            url,
        }
    }
}

/// `http://127.0.0.1:8081/repository/raw-main/` → `127.0.0.1:8081`.
#[must_use]
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split('/').next().unwrap_or(rest).to_owned()
}

/// Where downloads land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadCfg {
    /// The download directory, relative to the working directory of `nxr-tui`.
    #[serde(default = "default_download_dir")]
    pub dir: PathBuf,
}

fn default_download_dir() -> PathBuf {
    PathBuf::from("nxr-tui-downloads")
}

impl Default for DownloadCfg {
    fn default() -> Self {
        Self {
            dir: default_download_dir(),
        }
    }
}

/// TUI behavior switches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TuiCfg {
    /// Let repositories of every format be opened, not only raw.
    #[serde(default)]
    pub all_formats: bool,
}

/// The whole config file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    /// The format version, for future migrations.
    #[serde(default = "default_version")]
    pub version: u8,
    /// Download defaults.
    #[serde(default)]
    pub download: DownloadCfg,
    /// TUI defaults.
    #[serde(default)]
    pub tui: TuiCfg,
    /// Server presets, serialized as the `[[server]]` array of tables.
    #[serde(default, rename = "server", skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerCfg>,
}

fn default_version() -> u8 {
    1
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            download: DownloadCfg::default(),
            tui: TuiCfg::default(),
            servers: Vec::new(),
        }
    }
}

/// The config path this invocation uses: `--config` wins, then XDG, then HOME.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when no flag was passed and no config directory
/// can be derived from the environment.
pub fn target_path(explicit: Option<PathBuf>) -> Result<PathBuf, Error> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    default_path()
        .ok_or_else(|| Error::Misuse("no config directory: set XDG_CONFIG_HOME or HOME".into()))
}

/// The default config path: `$XDG_CONFIG_HOME/nxr-tui/config.toml`,
/// else `$APPDATA/nxr-tui/config.toml` on Windows,
/// else `$HOME/.config/nxr-tui/config.toml`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(Path::new(&xdg).join("nxr-tui").join("config.toml"));
        }
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return Some(Path::new(&appdata).join("nxr-tui").join("config.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| {
            Path::new(&h)
                .join(".config")
                .join("nxr-tui")
                .join("config.toml")
        })
}

/// Loads the config file.
/// A missing file is the default config; a broken file is a misuse error.
///
/// # Errors
///
/// Returns [`Error::Io`] when the file cannot be read and [`Error::Misuse`]
/// when the content is not valid TOML for this schema.
pub fn load(path: &Path) -> Result<ConfigFile, Error> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ConfigFile::default()),
        Err(e) => {
            return Err(Error::Io {
                path: path.display().to_string(),
                detail: e.to_string(),
            });
        }
    };
    toml::from_slice(&bytes).map_err(|e| Error::Misuse(format!("config {}: {e}", path.display())))
}

/// Writes the config as TOML under the explaining header, creating parents.
///
/// # Errors
///
/// Returns [`Error::Io`] when the directories or the file cannot be written.
pub fn save(path: &Path, cfg: &ConfigFile) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_of(dir))?;
    }
    let body = toml::to_string_pretty(cfg).map_err(|e| Error::Misuse(format!("config: {e}")))?;
    std::fs::write(path, format!("{HEADER}\n\n{body}")).map_err(io_of(path))
}

/// `--init-config`: writes the commented template, refusing to overwrite.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when the file already exists and [`Error::Io`]
/// when it cannot be written.
pub fn init(path: &Path) -> Result<(), Error> {
    if path.exists() {
        return Err(Error::Misuse(format!(
            "refusing to overwrite {}",
            path.display()
        )));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_of(dir))?;
    }
    std::fs::write(path, TEMPLATE).map_err(io_of(path))
}

/// The filesystem error of `path` as a core [`Error::Io`]: the variant's
/// fields are public, so wrappers construct it directly.
fn io_of(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::Io {
        path: path.display().to_string(),
        detail: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_parses_and_carries_one_preset() {
        let cfg: ConfigFile = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.download.dir, PathBuf::from("nxr-tui-downloads"));
        assert!(!cfg.tui.all_formats);
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].name, "main");
        assert_eq!(cfg.servers[0].url, "http://127.0.0.1:8081/");
    }

    #[test]
    fn host_of_strips_scheme_and_path() {
        assert_eq!(host_of("http://127.0.0.1:8081/"), "127.0.0.1:8081");
        assert_eq!(
            host_of("https://nexus.example.com/repository/raw-main/"),
            "nexus.example.com"
        );
    }

    #[test]
    fn a_missing_config_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = load(&dir.path().join("nope/config.toml")).unwrap();
        assert_eq!(cfg, ConfigFile::default());
    }

    #[test]
    fn save_and_load_roundtrip_preserve_presets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nxr-tui").join("config.toml");
        let mut cfg = ConfigFile::default();
        cfg.tui.all_formats = true;
        cfg.servers.push(ServerCfg {
            name: "backup".into(),
            url: "https://nexus.example.com/".into(),
        });
        save(&path, &cfg).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# nxr-tui config"), "header kept: {text}");
        assert_eq!(load(&path).unwrap(), cfg);
    }

    #[test]
    fn a_broken_config_is_misuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, b"version = not a number").unwrap();
        let err = load(&path).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn init_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        init(&path).unwrap();
        assert!(path.exists());
        assert_eq!(init(&path).unwrap_err().exit_code(), 2);
    }

    #[test]
    fn an_explicit_config_path_always_wins() {
        assert_eq!(
            target_path(Some(PathBuf::from("/tmp/x.toml"))).unwrap(),
            PathBuf::from("/tmp/x.toml")
        );
    }
}
