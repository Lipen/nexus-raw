//! Per-invocation configuration: no config file, no profiles (spec §4).
//!
//! Every call is self-sufficient: the base URL comes from the command line,
//! credentials from `-u` or env (see [`crate::creds`]).

use std::time::Duration;

use crate::error::Error;

/// The full core configuration for one invocation.
#[derive(Debug, Clone)]
pub struct Config {
    /// Base URL of the directory this command works on.
    /// `up`/`down`/`ls` take a directory URL; primitives take full object URLs.
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

impl Config {
    /// The sane check: workers in 1..=64, positive timeouts.
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
pub fn normalize_base(base: &str) -> Result<String, Error> {
    let err = || {
        Error::misuse(format!(
            "base URL {base:?} must be http(s)://host/path ending with /"
        ))
    };
    let url = reqwest::Url::parse(base).map_err(|_| err())?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(err());
    }
    // Credentials in the URL userinfo leak into errors, logs and doctor
    // output. The only homes for credentials are `-u` and the environment.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::misuse(
            "credentials in the URL userinfo would leak into errors and logs; \
             pass -u user:pass or export NXR_AUTH instead",
        ));
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
    fn normalization() {
        assert_eq!(normalize_base("http://h/a").unwrap(), "http://h/a/");
        assert!(normalize_base("http://h/a?x=1").is_err());
        assert!(normalize_base("notaurl").is_err());
        // NXR-01: credentials never travel in the URL.
        assert!(normalize_base("https://user:pass@host/repo/").is_err());
        assert!(normalize_base("https://@host/repo/").is_ok());
        assert!(normalize_base("https://:pass@host/repo/").is_err());
    }
}
