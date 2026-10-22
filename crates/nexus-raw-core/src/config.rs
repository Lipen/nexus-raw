//! Per-invocation configuration: no config file, no profiles (spec §4).
//!
//! Every call is self-sufficient: the base URL comes from the command line, credentials from `-u` or env (see [`crate::creds`]).

use std::time::Duration;

use crate::error::Error;

/// The full core configuration for one invocation.
#[derive(Clone)]
pub struct Config {
    /// Base URL of the directory this command works on.
    /// `up`/`down`/`ls` take a directory URL.
    /// Primitives take full object URLs.
    pub base: String,
    /// Skip TLS certificate verification.
    pub tls_insecure: bool,
    /// Parallel artifact transfers.
    pub workers: usize,
    /// Attempts per HTTP request.
    pub retry_attempts: u32,
    /// TCP connect timeout.
    pub connect_timeout: Duration,
    /// Fail a transfer when no bytes move for this long.
    pub stall_timeout: Duration,
    /// The `Authorization` header value, resolved from `-u` or env.
    pub auth: Option<String>,
}

impl std::fmt::Debug for Config {
    /// `auth` is the credential: it renders as `***`, never as the value.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base", &self.base)
            .field("tls_insecure", &self.tls_insecure)
            .field("workers", &self.workers)
            .field("retry_attempts", &self.retry_attempts)
            .field("connect_timeout", &self.connect_timeout)
            .field("stall_timeout", &self.stall_timeout)
            .field("auth", &self.auth.as_ref().map(|_| "***"))
            .finish()
    }
}

impl Config {
    /// The sane check: workers in 1..=64, positive timeouts.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when `workers` is outside `1..=64`, `retry_attempts` is zero, or a timeout is zero.
    pub fn validate(&self) -> Result<(), Error> {
        if self.workers == 0 || self.workers > 64 {
            return Err(Error::misuse(format!(
                "--workers must be in 1..=64, got {}",
                self.workers
            )));
        }
        if self.retry_attempts == 0 {
            return Err(Error::misuse("--retry must be at least 1"));
        }
        if self.connect_timeout.is_zero() || self.stall_timeout.is_zero() {
            return Err(Error::misuse("timeouts must be positive"));
        }
        Ok(())
    }

    /// The base URL, normalized (trailing `/`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when [`Self::base`] is not a valid http(s) base URL (see [`normalize_base`]).
    pub fn normalized_base(&self) -> Result<String, Error> {
        normalize_base(&self.base)
    }
}

/// Base URL normalization: http/https scheme, a host, trailing `/`, no query/fragment.
///
/// ```rust
/// use nexus_raw_core::config::normalize_base;
/// assert_eq!(
///     normalize_base("https://host/repository/raw").unwrap(),
///     "https://host/repository/raw/"
/// );
/// assert!(normalize_base("ftp://host/raw/").is_err());
/// // Credentials in the userinfo are rejected: they leak into output.
/// assert!(normalize_base("https://user:pass@host/raw/").is_err());
/// ```
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `base` is not an http(s) URL with a host and without userinfo, query or fragment.
pub fn normalize_base(base: &str) -> Result<String, Error> {
    // Never echo the raw base: a parse failure or a query token is exactly
    // where a secret would leak into stderr and the `--json` error object.
    let err = || {
        Error::misuse(
            "the base URL must be http(s)://host/path; credentials belong in -u or the environment, \
             and a query or fragment is refused",
        )
    };
    let url = reqwest::Url::parse(base).map_err(|_| err())?;
    // Credentials in the URL userinfo leak into errors, logs and doctor output.
    // The only homes for credentials are `-u` and the environment.
    // This check runs before any reject that echoes `{base}` back.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::misuse(
            "credentials in the URL userinfo would leak into errors and logs; \
             pass -u user:pass or export NXR_AUTH instead",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(err());
    }
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(err());
    }
    if url.host_str().is_none() {
        return Err(err());
    }
    let mut out = url.as_str().to_owned();
    if !out.ends_with('/') {
        out.push('/');
    }
    Ok(out)
}

/// Split a URL that ends at a file into its parent directory and the name.
/// `…/a/b.bin` -> (`…/a/`, `b.bin`).
/// A trailing slash means a directory, and a URL without a path is not a file.
#[must_use]
pub fn split_file_url(url: &str) -> Option<(String, String)> {
    let scheme_end = url.find("://")? + 3;
    let path_start = scheme_end + url[scheme_end..].find('/')?;
    let path = &url[path_start..];
    if path.ends_with('/') {
        return None;
    }
    let (parent, leaf) = path.rsplit_once('/')?;
    if leaf.is_empty() {
        return None;
    }
    Some((format!("{}{parent}/", &url[..path_start]), leaf.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(base: &str) -> Config {
        Config {
            base: base.to_owned(),
            tls_insecure: false,
            workers: 8,
            retry_attempts: 4,
            connect_timeout: Duration::from_secs(15),
            stall_timeout: Duration::from_secs(30),
            auth: None,
        }
    }

    #[test]
    fn validation_bounds() {
        assert!(cfg("http://h/").validate().is_ok());
        assert!(cfg("http://h/").validate().is_ok());
        let mut bad = cfg("http://h/");
        bad.workers = 0;
        assert!(bad.validate().is_err());
        bad.workers = 65;
        assert!(bad.validate().is_err());
        bad.workers = 8;
        bad.retry_attempts = 0;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn errors_never_echo_a_credentialed_url() {
        for bad in [
            "https://alice:s3cret@",
            "https://alice:s3cret@host:99999/",
            "https://host/repo?token=s3cret",
            "https://host/repo#s3cret",
        ] {
            let text = normalize_base(bad).unwrap_err().to_string();
            assert!(!text.contains("s3cret"), "{bad}: {text}");
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize_base("http://h/a").unwrap(), "http://h/a/");
        assert!(normalize_base("http://h/a?x=1").is_err());
        assert!(normalize_base("notaurl").is_err());
        // Credentials never travel in the URL, and no reject message echoes them back.
        // The literal `user:pass` is reserved for the remedy hint, so the test credentials differ.
        let leaked = normalize_base("https://alice:s3cret@host/repo/?x=1")
            .expect_err("userinfo must be rejected")
            .to_string();
        assert!(
            !leaked.contains("alice") && !leaked.contains("s3cret"),
            "the rejection must not echo the credentials: {leaked}"
        );
        assert!(normalize_base("https://alice:s3cret@host/repo/").is_err());
        assert!(normalize_base("https://@host/repo/").is_ok());
        assert!(normalize_base("https://:pass@host/repo/").is_err());
    }

    #[test]
    fn debug_redacts_auth() {
        let mut c = cfg("https://h/");
        c.auth = Some("Basic c2VjcmV0".to_owned());
        let text = format!("{c:?}");
        assert!(!text.contains("c2VjcmV0"), "leak: {text}");
        assert!(text.contains("***"), "redacted: {text}");
    }
    #[test]
    fn file_url_splits_at_its_parent() {
        assert_eq!(
            split_file_url("https://h/repository/raw/1.4.0/a.zip"),
            Some(("https://h/repository/raw/1.4.0/".into(), "a.zip".into()))
        );
        assert_eq!(
            split_file_url("https://h/b.bin"),
            Some(("https://h/".into(), "b.bin".into()))
        );
        assert_eq!(
            split_file_url("https://h/a/b/c.bin"),
            Some(("https://h/a/b/".into(), "c.bin".into()))
        );
    }

    #[test]
    fn directory_and_bare_urls_are_not_files() {
        for url in [
            "https://h/repository/raw/1.4.0/",
            "https://h/",
            "https://h",
            "https://h:8081",
        ] {
            assert_eq!(split_file_url(url), None, "{url}");
        }
    }
}
