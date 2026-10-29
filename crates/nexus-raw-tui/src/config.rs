//! The `nxr-tui` config file: server presets and defaults.
//!
//! The path is `$XDG_CONFIG_HOME/nxr-tui/config.toml`, falling back to
//! `$HOME/.config/nxr-tui/config.toml` (`--config` overrides both).
//! A missing file is the default config, so the TUI works with nothing on disk.
//! Passwords never live here: credentials come from `-u` or the environment at
//! every launch, exactly like the `nxr` CLI.

use std::path::{Path, PathBuf};

use nexus_raw_core::service::RepoInfo;
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
# Copy through the OSC 52 terminal escape before the external tools.
osc52 = true
# Left/right navigation in the tree:
#   \"enter\"  - right enters a folder, left goes back up;
#   \"expand\" - right expands a folder inline, left collapses it or jumps to the parent.
# Toggled inside the TUI with `e`.
nav = \"enter\"

# Every server is a preset: open it with `nxr-tui --server <name>`,
# or with no arguments at all. The `s` overlay inside the TUI adds one here.
[[server]]
name = \"main\"
url = \"http://127.0.0.1:8081/\"
";

/// One server preset: a label and a server root URL.
/// The label names the tab. The URL is what every request resolves against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCfg {
    /// The preset name and tab label.
    pub name: String,
    /// The server root URL, e.g. `http://127.0.0.1:8081/`.
    pub url: String,
}

impl ServerCfg {
    /// Whether `url` shares this server's origin.
    ///
    /// A server-supplied repository URL on another host is never dialed with the
    /// session credentials. A URL without a parseable origin (userinfo, a query
    /// or a fragment in it) shares nothing: the comparison fails closed.
    #[must_use]
    pub fn same_origin(&self, url: &str) -> bool {
        match (origin_of(&self.url), origin_of(url)) {
            (Some(server), Some(repo)) => server == repo,
            _ => false,
        }
    }

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

/// The scheme, host and port of a URL, lowercased, without the userinfo.
///
/// Parsed through the core's `normalize_base`, which refuses a URL carrying
/// userinfo, a query or a fragment: such a URL has no origin at all, so
/// [`ServerCfg::same_origin`] denies it instead of comparing a clever string.
/// A default port (`:80`, `:443`) is dropped, so `http://host:80/` and
/// `http://host/` name one origin.
#[must_use]
pub fn origin_of(url: &str) -> Option<String> {
    let base = nexus_raw_core::config::normalize_base(url).ok()?;
    let (scheme, rest) = base.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    let authority = rest.split('/').next()?;
    let (host, port) = split_host_port(authority);
    let host = host.to_ascii_lowercase();
    let default = matches!(
        (&scheme[..], port),
        ("http", Some("80")) | ("https", Some("443"))
    );
    Some(match (port, default) {
        (Some(port), false) => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    })
}

/// Split `host[:port]` from an authority, keeping an IPv6 literal in one piece.
fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    if let Some(close) = authority.rfind(']') {
        let host = &authority[..=close];
        return (host, authority[close + 1..].strip_prefix(':'));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    }
}

/// Whether a server-supplied repository name is safe as one local directory name.
///
/// The name becomes `{destination}/{name}`, and the smoke run deletes that
/// directory before downloading: only one ordinary path component passes.
/// An absolute name (`/home/user/.ssh`) would replace the destination and a
/// `..` component would walk out of it, so both are refused here.
/// The rule is containment, not the artifact grammar: a name with a space or
/// a non-ASCII letter joins literally and stays inside the destination.
#[must_use]
pub fn safe_dir_name(name: &str) -> bool {
    let path = std::path::Path::new(name);
    // Win32 strips trailing spaces and dots from a component, so `.. ` or `a.`
    // names something other than it reads: only a name that survives verbatim is
    // safe, on every platform.
    let trimmed = name.trim_end_matches(['.', ' ']);
    !name.is_empty()
        && trimmed.len() == name.len()
        && !trimmed.is_empty()
        && name.len() <= 255
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
        && !path.is_absolute()
        && path.components().count() == 1
        && matches!(
            path.components().next(),
            Some(std::path::Component::Normal(_))
        )
}

/// The repositories this session may show and dial: same origin, safe name.
///
/// A compromised server can name a repository on another origin or a name that
/// would escape the destination; both are dropped at ingest and again on every
/// refresh, which is the only filter the download verbs rely on.
#[must_use]
pub fn acceptable_repos(server: &ServerCfg, repos: Vec<RepoInfo>) -> Vec<RepoInfo> {
    repos
        .into_iter()
        .filter(|repo| server.same_origin(&repo.url) && safe_dir_name(&repo.name))
        .collect()
}

/// The URL as the UI shows it: `scheme://host[:port]`, never the userinfo.
///
/// [`origin_of`] returns `None` for a URL the core refuses (userinfo, a query
/// or a fragment), and the bare host is the last resort: a password typed into
/// the URL never reaches the status line, the error modal or the smoke output.
#[must_use]
pub fn shown_url(url: &str) -> String {
    origin_of(url).unwrap_or_else(|| host_of(url))
}

/// `http://127.0.0.1:8081/repository/raw-main/` → `127.0.0.1:8081`.
/// The userinfo never reaches a name: `http://user:pass@host/` derives `host`,
/// so a password in the URL cannot end up in the tab bar, the status line or
/// error output.
#[must_use]
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or(rest);
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_owned()
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

/// How left/right arrows navigate the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Nav {
    /// Right enters a folder, left goes back up (the current listing only).
    #[default]
    Enter,
    /// Right expands a folder inline, left collapses it or jumps to the parent.
    Expand,
}

impl Nav {
    /// The other mode.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Enter => Self::Expand,
            Self::Expand => Self::Enter,
        }
    }

    /// The short label for the status line.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Expand => "expand",
        }
    }
}

fn default_true() -> bool {
    true
}

/// TUI behavior switches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiCfg {
    /// Let repositories of every format be opened, not only raw.
    #[serde(default)]
    pub all_formats: bool,
    /// The left/right navigation mode.
    #[serde(default)]
    pub nav: Nav,
    /// Copy through the OSC 52 terminal escape before the external tools.
    #[serde(default = "default_true")]
    pub osc52: bool,
}

impl Default for TuiCfg {
    fn default() -> Self {
        Self {
            all_formats: false,
            nav: Nav::default(),
            osc52: true,
        }
    }
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
/// A missing file is the default config. A broken file is a misuse error.
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
    #[test]
    fn same_origin_rejects_a_foreign_host() {
        let server = super::ServerCfg::from_base("http://127.0.0.1:8081/".to_owned());
        assert!(server.same_origin("http://127.0.0.1:8081/repository/raw-main/"));
        assert!(!server.same_origin("http://evil.example/repository/raw-main/"));
        assert!(!server.same_origin("http://127.0.0.1:9999/repository/raw-main/"));
        assert!(!server.same_origin("https://127.0.0.1:8081/repository/raw-main/"));
    }

    use super::*;

    #[test]
    fn same_origin_rejects_a_clever_url() {
        let server = ServerCfg::from_base("http://127.0.0.1:8081/".to_owned());
        // A userinfo, a query or a fragment can make a hostile URL compare
        // equal by string; the parsed origin fails closed instead.
        assert!(!server.same_origin("http://evil.example#@127.0.0.1:8081/repository/x/"));
        assert!(!server.same_origin("http://evil.example?@127.0.0.1:8081/repository/x/"));
        assert!(!server.same_origin("http://user:pass@127.0.0.1:8081/repository/x/"));
        assert!(!server.same_origin("not a url"));
    }

    #[test]
    fn a_default_port_is_one_origin() {
        let plain = ServerCfg::from_base("http://host/".to_owned());
        assert!(plain.same_origin("http://host:80/repository/x/"));
        let secure = ServerCfg::from_base("https://host/".to_owned());
        assert!(secure.same_origin("https://host:443/repository/x/"));
        assert!(!secure.same_origin("https://host:8443/repository/x/"));
        let literal = ServerCfg::from_base("http://[::1]:8081/".to_owned());
        assert!(literal.same_origin("http://[::1]:8081/repository/x/"));
        assert!(!literal.same_origin("http://[::1]:8082/repository/x/"));
    }

    #[test]
    fn shown_url_never_carries_the_userinfo() {
        assert_eq!(shown_url("http://host/repository/x/"), "http://host");
        // A URL the core refuses (userinfo, query, fragment) falls back to the
        // bare host: the password never reaches the screen.
        assert_eq!(
            shown_url("http://user:pass@host:8081/repository/x/"),
            "host:8081"
        );
        assert_eq!(shown_url("http://host/repository/x/?q=1"), "host");
    }

    #[test]
    fn safe_dir_name_takes_one_ordinary_component() {
        assert!(safe_dir_name("raw-main"));
        assert!(safe_dir_name("a.b_c-1"));
        // Spaces and non-ASCII join literally and stay inside the destination.
        assert!(safe_dir_name("a b"));
        assert!(safe_dir_name("данные"));
        // Absolute and dot components would escape the destination.
        assert!(!safe_dir_name("/home/user/.ssh"));
        assert!(!safe_dir_name(".."));
        assert!(!safe_dir_name("."));
        assert!(!safe_dir_name("a/b"));
        assert!(!safe_dir_name("a\\b"));
        assert!(!safe_dir_name("a\nb"));
        // Win32 strips trailing spaces and dots from a component: `.. ` would
        // name the parent, `a.` would name `a`.
        assert!(!safe_dir_name(".. "));
        assert!(!safe_dir_name("..."));
        assert!(!safe_dir_name(" "));
        assert!(!safe_dir_name("a."));
        assert!(!safe_dir_name("a "));
        assert!(!safe_dir_name(""));
        assert!(!safe_dir_name(&"a".repeat(256)));
    }

    #[test]
    fn acceptable_repos_drops_a_hostile_name() {
        let server = ServerCfg::from_base("http://127.0.0.1:8081/".to_owned());
        let ok = RepoInfo {
            name: "raw-main".into(),
            format: "raw".into(),
            kind: "hosted".into(),
            url: "http://127.0.0.1:8081/repository/raw-main/".into(),
        };
        let escaping = RepoInfo {
            name: "/home/user/.ssh".into(),
            ..ok.clone()
        };
        assert_eq!(acceptable_repos(&server, vec![ok]).len(), 1);
        assert!(
            acceptable_repos(&server, vec![escaping]).is_empty(),
            "an unsafe name is dropped at ingest"
        );
    }

    #[test]
    fn the_template_parses_and_carries_one_preset() {
        let cfg: ConfigFile = toml::from_str(TEMPLATE).unwrap();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.download.dir, PathBuf::from("nxr-tui-downloads"));
        assert!(!cfg.tui.all_formats);
        assert!(cfg.tui.osc52);
        assert_eq!(cfg.tui.nav, Nav::Enter);
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].name, "main");
        assert_eq!(cfg.servers[0].url, "http://127.0.0.1:8081/");
    }

    #[test]
    fn a_config_without_the_new_keys_parses() {
        // A pre-osc52 config: every key added later must fall back to its default.
        let old = "\
version = 1

[download]
dir = \"dl\"

[tui]
all_formats = false
nav = \"expand\"

[[server]]
name = \"main\"
url = \"http://127.0.0.1:8081/\"
";
        let cfg: ConfigFile = toml::from_str(old).unwrap();
        assert_eq!(cfg.download.dir, PathBuf::from("dl"));
        assert!(cfg.tui.osc52, "osc52 defaults to true");
        assert_eq!(cfg.tui.nav, Nav::Expand);
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
    fn host_of_never_carries_the_userinfo() {
        assert_eq!(
            host_of("http://user:pass@127.0.0.1:8081/"),
            "127.0.0.1:8081"
        );
        assert_eq!(host_of("http://user@host/"), "host");
        assert_eq!(host_of("http://user:pass@host/"), "host");
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
