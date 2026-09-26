//! The mapping layer: plain-Rust conversions between Node-facing options and
//! the core API, unit-testable without a Node process.
//!
//! Everything here is pure data plumbing: opts → [`Config`], [`Event`] → JSON,
//! [`Error`] → `{ exitCode, hint }`. The `#[napi]` exports in `lib.rs` stay thin.

use std::time::Duration;

use nexus_raw_core::creds;
use nexus_raw_core::{Config, Error, Event};

/// CLI-mirroring defaults (`nxr --help`): parallel transfers, attempts, timeouts.
pub const DEFAULT_WORKERS: u32 = 8;
pub const DEFAULT_RETRY: u32 = 4;
pub const DEFAULT_CONNECT_TIMEOUT_MS: u32 = 15_000;
pub const DEFAULT_STALL_MS: u32 = 30_000;

/// The common option fields every command accepts.
///
/// `auth` wins over the env fallback (`NXR_AUTH`, `NXR_USERNAME` + `NXR_PASSWORD`),
/// mirroring the `-u` flag order in `creds::resolve`.
#[derive(Debug, Clone, Default)]
pub struct CommonOpts {
    pub auth_user: Option<String>,
    pub auth_pass: Option<String>,
    pub workers: Option<u32>,
    pub retry: Option<u32>,
    pub connect_timeout_ms: Option<u32>,
    pub stall_ms: Option<u32>,
    pub tls_insecure: Option<bool>,
}

/// Build the per-invocation core config from a base URL plus common options.
///
/// The base URL is always in argv (zero config); the placeholder base for
/// network-free commands is the caller's business.
pub fn build_config(base: &str, common: &CommonOpts) -> Result<Config, Error> {
    let explicit = match (&common.auth_user, &common.auth_pass) {
        (Some(u), Some(p)) => Some((u.as_str(), p.as_str())),
        (Some(_), None) | (None, Some(_)) => {
            return Err(Error::Misuse("auth needs user and pass together".into()))
        }
        (None, None) => None,
    };
    let cfg = Config {
        base: base.to_owned(),
        tls_insecure: common.tls_insecure.unwrap_or(false),
        workers: usize::try_from(common.workers.unwrap_or(DEFAULT_WORKERS))
            .map_err(|_| Error::Misuse("workers must be a small positive number".into()))?,
        retry_attempts: common.retry.unwrap_or(DEFAULT_RETRY),
        connect_timeout: Duration::from_millis(u64::from(
            common
                .connect_timeout_ms
                .unwrap_or(DEFAULT_CONNECT_TIMEOUT_MS),
        )),
        stall_timeout: Duration::from_millis(u64::from(
            common.stall_ms.unwrap_or(DEFAULT_STALL_MS),
        )),
        auth: creds::resolve(explicit)?.map(|c| c.header),
    };
    cfg.validate()?;
    Ok(cfg)
}

/// The JS-facing rejection payload of a core error.
///
/// `message` carries the hint as a trailing `hint: ` line so even a raw
/// consumer of the addon binary sees the full CLI-style text; the JS entry
/// strips that line and promotes it to the `hint` property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorPayload {
    /// The error code string on the rejected `Error`: `NXR_EXIT_<exit_code>`.
    pub code: String,
    pub message: String,
    pub exit_code: u8,
    pub hint: Option<String>,
}

pub fn error_payload(e: &Error) -> ErrorPayload {
    let hint = e.hint();
    let exit_code = e.exit_code();
    let mut message = e.to_string();
    message.push_str(&format!("\nnxr:exit {exit_code}"));
    if let Some(h) = &hint {
        message.push_str("\nhint: ");
        message.push_str(h);
    }
    ErrorPayload {
        code: format!("NXR_EXIT_{exit_code}"),
        message,
        exit_code,
        hint,
    }
}

/// The event's JSON form, exactly the NDJSON shapes the CLI emits.
pub fn event_to_json(event: &Event) -> serde_json::Value {
    event.to_json()
}

/// Classify a `manifest` spec: `-` for stdin, http(s) through the server,
/// everything else a local file — the CLI `load_manifest` rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestSpec {
    Stdin,
    Url(String),
    File(String),
}

pub fn classify_manifest_spec(spec: &str) -> ManifestSpec {
    match spec {
        "-" => ManifestSpec::Stdin,
        u if u.starts_with("http://") || u.starts_with("https://") => {
            ManifestSpec::Url(u.to_owned())
        }
        p => ManifestSpec::File(p.to_owned()),
    }
}

/// Whether a `sha` target is a URL or a local path (CLI `sha` dispatch).
pub fn is_url_target(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_raw_core::Summary;

    #[test]
    fn defaults_match_the_cli() {
        let cfg = build_config("http://host/repo/", &CommonOpts::default()).unwrap();
        assert_eq!(cfg.workers, 8);
        assert_eq!(cfg.retry_attempts, 4);
        assert_eq!(cfg.connect_timeout, Duration::from_millis(15_000));
        assert_eq!(cfg.stall_timeout, Duration::from_millis(30_000));
        assert!(!cfg.tls_insecure);
        assert!(cfg.auth.is_none());
    }

    #[test]
    fn explicit_opts_override_defaults() {
        let common = CommonOpts {
            workers: Some(16),
            retry: Some(1),
            connect_timeout_ms: Some(2_500),
            stall_ms: Some(60_000),
            tls_insecure: Some(true),
            ..Default::default()
        };
        let cfg = build_config("http://host/repo", &common).unwrap();
        assert_eq!(cfg.workers, 16);
        assert_eq!(cfg.retry_attempts, 1);
        assert_eq!(cfg.connect_timeout, Duration::from_millis(2_500));
        assert_eq!(cfg.stall_timeout, Duration::from_millis(60_000));
        assert!(cfg.tls_insecure);
        assert_eq!(cfg.normalized_base().unwrap(), "http://host/repo/");
    }

    #[test]
    fn explicit_auth_wins_over_env() {
        std::env::set_var("NXR_AUTH", "envB64");
        let common = CommonOpts {
            auth_user: Some("user".into()),
            auth_pass: Some("pass".into()),
            ..Default::default()
        };
        let cfg = build_config("http://host/", &common).unwrap();
        assert_eq!(cfg.auth.as_deref(), Some("Basic dXNlcjpwYXNz"));
        std::env::remove_var("NXR_AUTH");
    }

    #[test]
    fn env_fallback_applies_without_explicit_auth() {
        std::env::set_var("NXR_USERNAME", "user");
        std::env::set_var("NXR_PASSWORD", "pass");
        let cfg = build_config("http://host/", &CommonOpts::default()).unwrap();
        assert_eq!(cfg.auth.as_deref(), Some("Basic dXNlcjpwYXNz"));
        std::env::remove_var("NXR_USERNAME");
        std::env::remove_var("NXR_PASSWORD");
    }

    #[test]
    fn half_auth_is_misuse() {
        let common = CommonOpts {
            auth_user: Some("user".into()),
            auth_pass: None,
            ..Default::default()
        };
        let err = build_config("http://host/", &common).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn invalid_workers_refuse_with_misuse() {
        let common = CommonOpts {
            workers: Some(0),
            ..Default::default()
        };
        let err = build_config("http://host/", &common).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.hint().is_some());
    }

    #[test]
    fn error_payload_carries_code_exit_and_hint() {
        let p = error_payload(&Error::Misuse("bad flags".into()));
        assert_eq!(p.code, "NXR_EXIT_2");
        assert_eq!(p.exit_code, 2);
        assert_eq!(p.hint, Some("check the command line arguments".into()));
        assert!(p.message.contains("misuse: bad flags"));
        assert!(p.message.contains("\nnxr:exit 2\n"));
        assert!(p
            .message
            .ends_with("hint: check the command line arguments"));

        let p = error_payload(&Error::Enumerate {
            url: "http://host/raw/".into(),
            reason: "no source".into(),
        });
        assert_eq!(p.code, "NXR_EXIT_1");
        assert_eq!(p.exit_code, 1);
        assert!(p.hint.unwrap().contains("--manifest"));

        let p = error_payload(&Error::Transport {
            url: "http://host/raw/".into(),
            detail: "timeout".into(),
        });
        assert_eq!(p.code, "NXR_EXIT_3");
        assert_eq!(p.exit_code, 3);
    }

    #[test]
    fn events_render_the_ndjson_shapes() {
        let plan = event_to_json(&Event::Plan {
            upload: vec!["a".into()],
            download: vec![],
            skip: vec!["b".into()],
        });
        assert_eq!(plan["event"], "plan");
        assert_eq!(plan["upload"], serde_json::json!(["a"]));
        assert_eq!(plan["skip"], serde_json::json!(["b"]));

        let bytes = event_to_json(&Event::ArtifactBytes {
            name: "a".into(),
            dir: nexus_raw_core::Dir::Down,
            done: 12,
            total: Some(34),
        });
        assert_eq!(bytes["event"], "artifact");
        assert_eq!(bytes["state"], "downloading");
        assert_eq!(bytes["done"], 12);
        assert_eq!(bytes["total"], 34);

        let retry = event_to_json(&Event::Retrying {
            name: "a".into(),
            attempt: 2,
            reason: "reset".into(),
        });
        assert_eq!(retry["event"], "retrying");
        assert_eq!(retry["attempt"], 2);
    }

    #[test]
    fn summary_event_is_the_transfer_promise_shape() {
        let summary = Summary {
            uploaded: 1,
            downloaded: 2,
            skipped: 3,
            failed: vec!["c".into()],
        };
        let line = event_to_json(&Event::Summary(summary));
        assert_eq!(
            line,
            serde_json::json!({"event": "summary", "uploaded": 1, "downloaded": 2, "skipped": 3, "failed": ["c"]})
        );
    }

    #[test]
    fn manifest_specs_classify_like_the_cli() {
        assert_eq!(classify_manifest_spec("-"), ManifestSpec::Stdin);
        assert_eq!(
            classify_manifest_spec("https://host/repo/manifest.json"),
            ManifestSpec::Url("https://host/repo/manifest.json".into())
        );
        assert_eq!(
            classify_manifest_spec("http://host/repo/manifest.json"),
            ManifestSpec::Url("http://host/repo/manifest.json".into())
        );
        assert_eq!(
            classify_manifest_spec("./manifest.json"),
            ManifestSpec::File("./manifest.json".into())
        );
    }

    #[test]
    fn sha_targets_split_by_scheme() {
        assert!(is_url_target("https://host/a.bin"));
        assert!(is_url_target("http://host/a.bin"));
        assert!(!is_url_target("./a.bin"));
        assert!(!is_url_target("/abs/a.bin"));
    }
}
