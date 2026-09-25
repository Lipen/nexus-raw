//! Error taxonomy from protocol §8.
//!
//! The single mapping from error to exit code lives in [`Error::exit_code`].

use std::fmt;

/// nexus-raw errors. The variant set mirrors the §8 table; fields are public
/// so wrappers can construct and match them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Digest mismatch on a completed artifact.
    #[error("mismatch: {name}: {detail}")]
    Mismatch {
        name: String,
        detail: String,
    },
    /// Names lacking completion after the work (or before it).
    #[error("incomplete: {}", .names.join(", "))]
    Incomplete { names: Vec<String> },
    /// Name rejected by the §3 grammar.
    #[error("unsafe name: {name}: {reason}")]
    UnsafeName {
        name: String,
        reason: String,
    },
    /// Remote claim differs from the local one (or does not parse).
    #[error("claim drift: version {version}: {detail}")]
    ClaimDrift {
        version: String,
        detail: String,
    },
    /// Claimed names exist neither locally nor remotely.
    #[error("missing: {}: claimed but exist nowhere", .names.join(", "))]
    Missing { names: Vec<String> },
    /// 401/403 or missing credentials when required.
    #[error("auth: {url}: {reason}")]
    Auth {
        url: String,
        reason: String,
    },
    /// Network, TLS, 5xx, timeout after retries.
    #[error("transport: {url}: {detail}")]
    Transport {
        url: String,
        detail: String,
    },
    /// Any other unexpected status.
    #[error("http {status}: {url}")]
    Http {
        status: u16,
        url: String,
    },
    /// Bad flags, missing file/directory, broken config.
    #[error("misuse: {0}")]
    Misuse(String),
}

impl Error {
    /// Exit code: 0 ok, 1 data, 2 misuse, 3 transport.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Mismatch { .. }
            | Error::Incomplete { .. }
            | Error::ClaimDrift { .. }
            | Error::Missing { .. } => 1,
            Error::UnsafeName { .. } | Error::Misuse(_) => 2,
            Error::Auth { .. } | Error::Transport { .. } | Error::Http { .. } => 3,
        }
    }

    pub(crate) fn misuse(msg: impl Into<String>) -> Self {
        Error::Misuse(msg.into())
    }

    pub(crate) fn transport(url: &str, source: impl fmt::Display) -> Self {
        Error::Transport {
            url: url.to_owned(),
            detail: source.to_string(),
        }
    }

    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Error::Misuse(format!("{}: {source}", path.display()))
    }
}

/// Symmetric diff refusal (§5.2). Always exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Verdict {
    #[error("mismatch: {name}: {detail}")]
    Mismatch { name: String, detail: String },
    #[error("missing: {}: claimed but exist nowhere", fmt_names(names))]
    Missing { names: Vec<String> },
    #[error("incomplete: {}: local copies are not Complete before publish", fmt_names(names))]
    LocalIncomplete { names: Vec<String> },
}

fn fmt_names(names: &[String]) -> String {
    names.join(", ")
}

impl From<Verdict> for Error {
    fn from(v: Verdict) -> Self {
        match v {
            Verdict::Mismatch { name, detail } => Error::Mismatch { name, detail },
            Verdict::Missing { names } => Error::Missing { names },
            Verdict::LocalIncomplete { names } => Error::Incomplete { names },
        }
    }
}
