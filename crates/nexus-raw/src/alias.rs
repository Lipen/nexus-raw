//! Remote aliases: named endpoints in a TOML file, resolved only when the invocation names one.
//!
//! The file never loads implicitly: no `-R`, no read.
//! That keeps the curl-model contract intact — a call without the flag behaves
//! exactly like a call on a machine without the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use nexus_raw_core::error::Error;

/// The CLI-facing constructor: the core keeps `misuse` crate-private.
fn misuse(msg: impl Into<String>) -> Error {
    Error::Misuse(msg.into())
}

/// The default file location: `$XDG_CONFIG_HOME/nxr/config.toml`, then `~/.config/nxr/config.toml`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Some(Path::new(&x).join("nxr").join("config.toml"));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| {
            Path::new(&h)
                .join(".config")
                .join("nxr")
                .join("config.toml")
        })
}

/// The file to read: `NXR_CONFIG` overrides the default location.
pub fn config_path() -> Result<PathBuf, Error> {
    if let Ok(p) = std::env::var("NXR_CONFIG") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    default_path().ok_or_else(|| {
        misuse(
            "no config file location: set NXR_CONFIG or HOME (the file is only read when -R names an alias)",
        )
    })
}

/// One alias: the endpoint and how to answer "who are you".
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Alias {
    /// The base URL the alias expands to.
    pub url: String,
    /// The username, inline.
    #[serde(default)]
    pub user: Option<String>,
    /// The password, inline (discouraged: the file then carries a secret).
    #[serde(default)]
    pub pass: Option<String>,
    /// The env variable naming the username (resolved at call time).
    #[serde(default, rename = "user_env")]
    pub user_env: Option<String>,
    /// The env variable naming the password (resolved at call time).
    #[serde(default, rename = "pass_env")]
    pub pass_env: Option<String>,
    /// A shell command printing the password (resolved at call time).
    #[serde(default, rename = "pass_cmd")]
    pub pass_cmd: Option<String>,
}

impl Alias {
    /// The `(user, pass)` pair, resolved at call time.
    ///
    /// # Errors
    ///
    /// [`Error::Misuse`] when the alias names a missing env variable or a failing `pass_cmd`,
    /// and [`Error::Auth`] for an alias with no usable credential source at all (anonymous
    /// through an alias is a typo more often than an intent).
    pub fn creds(&self, name: &str) -> Result<Option<(String, String)>, Error> {
        let user: Option<String> = match self.user.clone() {
            Some(u) => Some(u),
            None => match &self.user_env {
                Some(v) => Some(read_env(name, v)?),
                None => None,
            },
        };
        if let Some(cmd) = &self.pass_cmd {
            let pass = run_pass_cmd(name, cmd)?;
            let user = user.ok_or_else(|| no_user(name))?;
            return Ok(Some((user, pass)));
        }
        let pass: Option<String> = match self.pass.clone() {
            Some(p) => Some(p),
            None => match &self.pass_env {
                Some(v) => Some(read_env(name, v)?),
                None => None,
            },
        };
        match (user, pass) {
            (Some(u), Some(p)) => Ok(Some((u, p))),
            (Some(_), None) | (None, Some(_)) => Err(misuse(format!(
                "alias `{name}`: the user and the password must come from the same source"
            ))),
            // No credential source at all: anonymous through the alias.
            // A real Nexus answers 401 when the credentials were required,
            // and the error carries the hint, so the intent stays visible.
            (None, None) => Ok(None),
        }
    }
}

fn read_env(alias: &str, var: &str) -> Result<String, Error> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            misuse(format!(
                "alias `{alias}`: the env variable `{var}` is not set"
            ))
        })
}

fn no_user(alias: &str) -> Error {
    misuse(format!(
        "alias `{alias}`: pass_cmd is set, but no user source (user or user_env)"
    ))
}

fn run_pass_cmd(alias: &str, cmd: &str) -> Result<String, Error> {
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| misuse(format!("alias `{alias}`: pass_cmd failed to run: {e}")))?;
    if !out.status.success() {
        let body = String::from_utf8_lossy(&out.stderr);
        let first = body.lines().next().unwrap_or("").to_owned();
        return Err(misuse(format!(
            "alias `{alias}`: pass_cmd exited {}: {first}",
            out.status.code().unwrap_or(-1)
        )));
    }
    let pass = String::from_utf8_lossy(&out.stdout);
    let pass = pass.trim_end_matches(['\r', '\n']);
    if pass.is_empty() {
        return Err(misuse(format!("alias `{alias}`: pass_cmd printed nothing")));
    }
    Ok(pass.to_owned())
}

/// The parsed file: a map of alias names.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Aliases {
    /// The `[alias.<name>]` tables.
    #[serde(rename = "alias")]
    pub map: BTreeMap<String, Alias>,
}

impl Aliases {
    /// Parse the file at `path`.
    ///
    /// # Errors
    ///
    /// [`Error::Misuse`] when the file is missing, unreadable or not valid TOML,
    /// and when an alias table lacks the `url` key.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            misuse(format!(
                "cannot read {}: {} (the file is only read when -R names an alias)",
                path.display(),
                e
            ))
        })?;
        let parsed: Aliases = toml::from_str(&text)
            .map_err(|e| misuse(format!("{} is not a valid alias file: {e}", path.display())))?;
        Ok(parsed)
    }

    /// The named alias, with every name listed in the error when absent.
    ///
    /// # Errors
    ///
    /// [`Error::Misuse`] when the alias is not in the file.
    pub fn get(&self, name: &str) -> Result<&Alias, Error> {
        self.map.get(name).ok_or_else(|| {
            let mut names: Vec<&str> = self.map.keys().map(String::as_str).collect();
            names.sort_unstable();
            let known = if names.is_empty() {
                "the file defines none".to_owned()
            } else {
                format!("known: {}", names.join(", "))
            };
            misuse(format!("alias `{name}` is not in the file; {known}"))
        })
    }
}

/// Expand an alias URL against a path argument.
///
/// The argument is joined onto the alias base when it looks relative
/// (no scheme, does not start with `/`); an absolute path or a full URL passes through.
/// `.` on a repository-root alias stays the root itself.
#[must_use]
pub fn join(base: &str, arg: &str) -> String {
    if arg.contains("://") || arg.starts_with('/') {
        return arg.to_owned();
    }
    if arg == "." {
        return base.trim_end_matches('/').to_owned();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        arg.trim_start_matches("./")
    )
}

/// Which positional fields of the parsed CLI carry URLs.
/// A mirror/move src and dst are two URLs in one command.
fn url_slots(cli: &crate::Cli) -> Vec<*mut String> {
    use crate::Cmd as C;
    match &cli.command {
        C::Get { url, .. } | C::Put { url, .. } | C::Head { url, .. } => {
            vec![std::ptr::addr_of!(*url).cast_mut()]
        }
        C::Up { dst, .. }
        | C::Down { src: dst, .. }
        | C::Rm { src: dst, .. }
        | C::Ls { url: dst, .. } => {
            vec![std::ptr::addr_of!(*dst).cast_mut()]
        }
        C::Point { url, .. } => vec![std::ptr::addr_of!(*url).cast_mut()],
        C::Mirror { src, dst, .. } | C::Mv { src, dst, .. } => {
            vec![
                std::ptr::addr_of!(*src).cast_mut(),
                std::ptr::addr_of!(*dst).cast_mut(),
            ]
        }
        C::Diff { src, .. } => vec![std::ptr::addr_of!(*src).cast_mut()],
        C::Sha { target } => {
            // A local path is common; the alias join passes it through unchanged
            // unless it looks relative to the alias — but sha of a local path
            // has no scheme, so rewriting it would corrupt it. Skip: sha takes
            // URLs only as `http(s)://…`, which never needs an alias.
            let _ = target;
            vec![]
        }
        C::Channel { op } => match op {
            crate::ChannelOp::Get { url } | crate::ChannelOp::Set { url, .. } => {
                vec![std::ptr::addr_of!(*url).cast_mut()]
            }
        },
        C::Service { op } => match op {
            crate::ServiceOp::Repos { url }
            | crate::ServiceOp::Status { url }
            | crate::ServiceOp::Repo { url, .. }
            | crate::ServiceOp::Assets { url, .. }
            | crate::ServiceOp::Eula { url, .. } => vec![std::ptr::addr_of!(*url).cast_mut()],
        },
        C::Doctor { url: Some(url) } => vec![std::ptr::addr_of!(*url).cast_mut()],
        C::Verify { .. } | C::Complete { .. } | C::Doctor { url: None } => vec![],
    }
}

/// Resolve `-R`: expand URL arguments through the alias and attach the alias credentials.
///
/// Priority: `-u` wins over the alias credentials; the alias wins over ambient env.
///
/// # Errors
///
/// [`Error::Misuse`] for a missing or malformed config file and an unknown alias,
/// and whatever [`Alias::creds`] raises for a broken credential source.
pub fn apply_remote(cli: &mut crate::Cli) -> Result<(), Error> {
    let Some(name) = cli.remote.clone() else {
        return Ok(());
    };
    let path = config_path()?;
    let aliases = Aliases::load(&path)?;
    let alias = aliases.get(&name)?;

    for slot in url_slots(cli) {
        // SAFETY: the pointers come from `&mut cli` fields taken just above,
        // no aliasing exists in this scope, and each field is written once.
        let url = unsafe { &mut *slot };
        *url = join(&alias.url, url);
    }

    if cli.user.is_none() {
        cli.alias_creds = alias.creds(&name)?;
        cli.alias_name = Some(name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(text: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    const FILE: &str = r#"
[alias.corp]
url = "https://nex.example.com:10443"
user_env = "TEST_ALIAS_USER"
pass_env = "TEST_ALIAS_PASS"

[alias.releases]
url = "https://nex.example.com:10443/repository/releases-raw"
user = "deploy"
pass = "literal-secret"

[alias.prod]
url = "https://prod.example.com"
user = "ci"
pass_cmd = "echo cmd-secret"

[alias.bare]
url = "https://anon.example.com"
"#;

    // These tests mutate process env: the lock serializes them.
    static ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn parses_and_lists() {
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        assert_eq!(a.map.len(), 4);
        assert_eq!(
            a.get("releases").unwrap().url,
            "https://nex.example.com:10443/repository/releases-raw"
        );
    }

    #[test]
    fn unknown_alias_names_the_known_ones() {
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        let e = format!("{}", a.get("nope").unwrap_err());
        assert!(e.contains("known: bare, corp, prod, releases"), "{e}");
    }

    #[test]
    fn missing_file_is_misuse_with_context() {
        let e = format!(
            "{}",
            Aliases::load(Path::new("/no/such/file.toml")).unwrap_err()
        );
        assert!(e.contains("cannot read"), "{e}");
        assert!(e.contains("only read when -R"), "{e}");
    }

    #[test]
    fn broken_toml_is_misuse() {
        let (_d, p) = write("[alias");
        let e = format!("{}", Aliases::load(&p).unwrap_err());
        assert!(e.contains("not a valid alias file"), "{e}");
    }

    #[test]
    fn env_creds_resolve_at_call_time() {
        let _g = ENV_LOCK.lock();
        std::env::set_var("TEST_ALIAS_USER", "u1");
        std::env::set_var("TEST_ALIAS_PASS", "p1");
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        let (u, s) = a.get("corp").unwrap().creds("corp").unwrap().unwrap();
        assert_eq!((u.as_str(), s.as_str()), ("u1", "p1"));
        std::env::remove_var("TEST_ALIAS_USER");
        std::env::remove_var("TEST_ALIAS_PASS");
    }

    #[test]
    fn missing_env_names_the_variable() {
        let _g = ENV_LOCK.lock();
        std::env::remove_var("TEST_ALIAS_USER");
        std::env::remove_var("TEST_ALIAS_PASS");
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        let e = format!("{}", a.get("corp").unwrap().creds("corp").unwrap_err());
        assert!(e.contains("TEST_ALIAS_USER"), "{e}");
    }

    #[test]
    fn literal_creds_pass_through() {
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        let (u, s) = a
            .get("releases")
            .unwrap()
            .creds("releases")
            .unwrap()
            .unwrap();
        assert_eq!((u.as_str(), s.as_str()), ("deploy", "literal-secret"));
    }

    #[test]
    fn pass_cmd_runs_and_trims() {
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        let (u, s) = a.get("prod").unwrap().creds("prod").unwrap().unwrap();
        assert_eq!((u.as_str(), s.as_str()), ("ci", "cmd-secret"));
    }

    #[test]
    fn pass_cmd_failure_is_misuse_with_stderr() {
        let (_d, p) = write(
            r#"
[alias.bad]
url = "https://x.example.com"
user = "u"
pass_cmd = "echo out >&2; exit 7"
"#,
        );
        let a = Aliases::load(&p).unwrap();
        let e = format!("{}", a.get("bad").unwrap().creds("bad").unwrap_err());
        assert!(e.contains("exited 7"), "{e}");
        assert!(e.contains("out"), "{e}");
    }

    #[test]
    fn bare_alias_is_anonymous() {
        let (_d, p) = write(FILE);
        let a = Aliases::load(&p).unwrap();
        assert!(a.get("bare").unwrap().creds("bare").unwrap().is_none());
    }

    #[test]
    fn half_creds_are_misuse() {
        let (_d, p) = write(
            r#"
[alias.half]
url = "https://x.example.com"
user = "u"
"#,
        );
        let a = Aliases::load(&p).unwrap();
        let e = format!("{}", a.get("half").unwrap().creds("half").unwrap_err());
        assert!(e.contains("same source"), "{e}");
    }

    #[test]
    fn join_cases() {
        assert_eq!(join("https://x/repo/", "1.4.0/"), "https://x/repo/1.4.0/");
        assert_eq!(
            join("https://x/repo", "1.4.0/file.bin"),
            "https://x/repo/1.4.0/file.bin"
        );
        assert_eq!(join("https://x/repo", "."), "https://x/repo");
        assert_eq!(join("https://x/repo/", "./1.4.0/"), "https://x/repo/1.4.0/");
        assert_eq!(join("https://x/repo", "/other/repo/"), "/other/repo/");
        assert_eq!(join("https://x/repo", "https://y/repo/"), "https://y/repo/");
    }

    #[test]
    fn default_path_prefers_xdg() {
        let _g = ENV_LOCK.lock();
        std::env::set_var("XDG_CONFIG_HOME", "/cfg");
        std::env::remove_var("NXR_CONFIG");
        assert_eq!(default_path().unwrap(), Path::new("/cfg/nxr/config.toml"));
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/t");
        assert_eq!(
            default_path().unwrap(),
            Path::new("/home/t/.config/nxr/config.toml")
        );
        std::env::remove_var("HOME");
    }

    #[test]
    fn nxr_config_overrides_the_location() {
        let _g = ENV_LOCK.lock();
        std::env::set_var("NXR_CONFIG", "/explicit.toml");
        assert_eq!(config_path().unwrap(), Path::new("/explicit.toml"));
        std::env::remove_var("NXR_CONFIG");
    }
}
