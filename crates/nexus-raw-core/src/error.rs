//! Error taxonomy (spec §7).
//!
//! The single mapping from error to exit code lives in [`Error::exit_code`].
//! Every error carries a human hint: the CLI prints it to stderr and puts it
//! into the `hint` field of JSON output.

use std::fmt;

/// nexus-raw errors.
/// The variant set mirrors the §7 table.
/// Fields are public so wrappers can construct and match them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Digest mismatch on a completed artifact.
    #[error("mismatch: {name}: {detail}")]
    Mismatch { name: String, detail: String },
    /// Names lacking completion after the work (or before it).
    #[error("incomplete: {}", .names.join(", "))]
    Incomplete { names: Vec<String> },
    /// Name rejected by the grammar.
    #[error("unsafe name: {name}: {reason}")]
    UnsafeName { name: String, reason: String },
    /// Requested names exist neither locally nor remotely.
    #[error("missing: {}: exist nowhere", .names.join(", "))]
    Missing { names: Vec<String> },
    /// The server cannot enumerate the requested directory.
    #[error("cannot enumerate: {url}: {reason}")]
    Enumerate { url: String, reason: String },
    /// 401/403 or missing credentials when required.
    #[error("auth: {url}: {reason}")]
    Auth { url: String, reason: String },
    /// Network, TLS, 5xx, timeout after retries.
    #[error("transport: {url}: {detail}")]
    Transport { url: String, detail: String },
    /// Any other unexpected status.
    #[error("http {status}: {url}")]
    Http { status: u16, url: String },
    /// Bad flags, missing file or directory.
    #[error("misuse: {0}")]
    Misuse(String),
}

impl Error {
    /// Exit code: 0 ok, 1 data, 2 misuse, 3 transport.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Mismatch { .. }
            | Error::Incomplete { .. }
            | Error::Missing { .. }
            | Error::Enumerate { .. } => 1,
            Error::UnsafeName { .. } | Error::Misuse(_) => 2,
            Error::Auth { .. } | Error::Transport { .. } | Error::Http { .. } => 3,
        }
    }

    /// The human hint for this error class: what to check next.
    /// Rendered to stderr and into the JSON `hint` field (§5.4).
    pub fn hint(&self) -> Option<String> {
        match self {
            Error::Mismatch { .. } => Some(
                "the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object"
                    .into(),
            ),
            Error::Incomplete { .. } => Some("rerun the same command; finished names are skipped and the rest is retried".into()),
            Error::Missing { .. } => Some("the name is absent on both sides; check spelling and the manifest".into()),
            Error::Enumerate { .. } => Some(
                "pass --manifest <file|url|->, repeat --name, or use --ls when the server has the search API".into(),
            ),
            Error::UnsafeName { .. } => Some("names must be relative paths of [A-Za-z0-9._-] segments; the .sha256 suffix is reserved".into()),
            Error::Auth { .. } => {
                Some("pass -u user:pass or export NXR_AUTH (base64 user:pass)".into())
            }
            Error::Transport { .. } => Some("check the network; transfers are resumable, rerunning is safe".into()),
            Error::Http { status: 404, .. } => {
                Some("check the URL path and that the version or object exists".into())
            }
            Error::Http { .. } => None,
            Error::Misuse(_) => Some("check the command line arguments".into()),
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

/// Symmetric diff refusal (spec §5.2). Always exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Verdict {
    #[error("mismatch: {name}: {detail}")]
    Mismatch { name: String, detail: String },
    #[error("missing: {}: exist nowhere", fmt_names(names))]
    Missing { names: Vec<String> },
    #[error(
        "incomplete: {}: local copies are not Complete before verification",
        fmt_names(names)
    )]
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
