//! Error taxonomy (spec §7).
//!
//! The single mapping from error to exit code lives in [`Error::exit_code`].
//! Every error carries a human hint: the CLI prints it to stderr and puts it into the `hint` field of JSON output.

use std::fmt;

/// nexus-raw errors.
/// The variant set mirrors the §7 table.
/// Fields are public so wrappers can construct and match them.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Digest mismatch on a completed artifact.
    #[error("mismatch: {}: {detail}", name.escape_debug())]
    Mismatch { name: String, detail: String },
    /// Names lacking completion after the work (or before it).
    #[error("incomplete: {}", .names.join(", "))]
    Incomplete { names: Vec<String> },
    /// Name rejected by the grammar.
    /// The raw input is server- or user-controlled, so it renders escaped (no ANSI/OSC injection into the terminal).
    #[error("unsafe name: {}: {reason}", name.escape_debug())]
    UnsafeName { name: String, reason: String },
    /// Requested names exist neither locally nor remotely.
    #[error("missing: {}: exist nowhere", .names.join(", "))]
    Missing { names: Vec<String> },
    /// The server cannot enumerate the requested directory.
    #[error("cannot enumerate: {url}: {reason}")]
    Enumerate { url: String, reason: String },
    /// 401/403 or missing credentials when required.
    /// `detail` is the first line of the server's error body, when the response carried one (spec §5).
    #[error("auth: {url}: {reason}")]
    Auth {
        url: String,
        reason: String,
        detail: Option<String>,
    },
    /// Network, TLS, 5xx, timeout after retries.
    #[error("transport: {url}: {detail}")]
    Transport { url: String, detail: String },
    /// Any other unexpected status.
    #[error("http {status}: {url}")]
    Http { status: u16, url: String },
    /// The server's service REST API answered 404: not a Nexus, or a version without the endpoint.
    /// Storage invariants are unaffected.
    /// Only `service repos` needs this surface.
    #[error("http 404: {url}: the service API is absent")]
    ServiceMissing { url: String, root: String },
    /// The search API answered 400 to a repository-scoped listing: the repository is missing on the server or is not a raw repository.
    /// A real Nexus refuses an unknown repository with 400 before any storage is touched.
    #[error("http 400: {url}: the search refused the repository")]
    SearchRepoMissing { url: String },
    /// The repository refuses the deletion: a read-only deployment answers 403/405 to DELETE (§5.4).
    /// `detail` is the first line of the server's error body, when the response carried one (spec §5).
    #[error("read-only: {url}: HTTP {status}")]
    ReadOnly {
        url: String,
        status: u16,
        detail: Option<String>,
    },
    /// Bad flags, missing file or directory.
    #[error("misuse: {0}")]
    Misuse(String),
    /// A local filesystem failure (part files, markers, rename).
    /// Not misuse: the command line may be perfect and the disk full.
    #[error("io: {path}: {detail}")]
    Io { path: String, detail: String },
    /// A server answered a small-object GET with a body beyond the protocol's bound (§3).
    /// Not misuse: the command line may be perfect and the server hostile or broken.
    #[error("small object: {url} exceeds the small-object cap ({cap} bytes)")]
    SmallObjectCap { url: String, cap: u64 },
    /// A server body the protocol requires to be JSON (or the channel token text) did not parse.
    /// Not a divergent object: the wire shape itself is broken, so the class is transport, not data.
    #[error("server body: {url}: {detail}")]
    ServerBody { url: String, detail: String },
    /// The server answered a ranged GET with the whole body: `Range` is ignored, and a segmented download cannot proceed.
    /// A signal to the caller, not a verdict: the single-stream fallback answers it.
    #[error("range ignored: {url}: the server answered a ranged GET with the whole body")]
    RangeIgnored { url: String },
}

/// The exit code shared by every data verdict: mismatch, incomplete, missing,
/// enumerate, read-only, a storage 404 and local I/O.
pub(crate) const DATA_EXIT: u8 = 1;

impl Error {
    /// Exit code: 0 ok, 1 data, 2 misuse, 3 transport.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Mismatch { .. }
            | Error::Incomplete { .. }
            | Error::Missing { .. }
            | Error::Enumerate { .. }
            | Error::ReadOnly { .. } => DATA_EXIT,
            Error::UnsafeName { .. } | Error::Misuse(_) => 2,
            Error::Auth { .. } | Error::Transport { .. } => 3,
            // A storage 404 is a data fact: the object or version is absent,
            // and a retry cannot change it. Scripts branch on it (0 proceed,
            // 1 absent, 3 retry later), so it must not share the transport class.
            Error::Http { status: 404, .. } => DATA_EXIT,
            Error::Http { .. } => 3,
            Error::ServiceMissing { .. } | Error::SearchRepoMissing { .. } => 3,
            // A hostile or broken server is the transport's problem: a retry
            // on the same URL changes nothing, but the class is not the
            // user's command line either.
            Error::SmallObjectCap { .. } => 3,
            Error::ServerBody { .. } => 3,
            Error::RangeIgnored { .. } => 3,
            Error::Io { .. } => DATA_EXIT,
        }
    }

    /// The human hint for this error class: what to check next.
    /// Rendered to stderr and into the JSON `hint` field (§5.4).
    /// When the error carries the server's body, the hint ends with `server says: <first body line>` (spec §5).
    #[must_use]
    pub fn hint(&self) -> Option<String> {
        let hint = match self {
            Error::Mismatch { .. } => Some(
                "the two sides diverge: delete or fix one copy, never let nxr overwrite a diverging object"
                    .into(),
            ),
            Error::Incomplete { .. } => Some("rerun the same command: finished names are skipped and the rest is retried".into()),
            Error::Missing { .. } => Some("the name is absent on both sides: check spelling and the enumeration source".into()),
            Error::Enumerate { .. } => Some(
                "repeat --name, use --names-from FILE, or use --ls when the server has the search API".into(),
            ),
            Error::UnsafeName { .. } => Some("names must be relative paths of [A-Za-z0-9._-] segments: the .sha256 and .part suffixes are reserved".into()),
            Error::Auth { .. } => {
                Some("pass -u user:pass or export NXR_AUTH (base64 user:pass)".into())
            }
            Error::Transport { .. } => Some("check the network: transfers are resumable, rerunning is safe".into()),
            Error::ReadOnly { status, .. } => Some(
                format!(
                    "the repository answered {status} to DELETE: it is read-only or the credentials lack write access, so rerunning is safe and nothing was removed"
                ),
            ),
            Error::Http { status: 404, .. } => {
                Some("check the URL path and that the version or object exists".into())
            }
            Error::ServiceMissing { root, .. } => Some(
                format!("the service API lives at the server root: try {root}/service/rest/v1/repositories"),
            ),
            Error::SearchRepoMissing { .. } => {
                Some("the repository is missing on the server or is not a raw repository".into())
            }
            Error::Http { status, .. } => Some(
                format!("the server answered {status}: check the URL path and the server health"),
            ),
            Error::Misuse(_) => Some("check the command line arguments".into()),
            Error::RangeIgnored { .. } => Some(
                "the server does not honor range requests: the download fell back to a single stream".into(),
            ),
            Error::Io { .. } => {
                Some("check the local filesystem: permissions, space, symlinks. Transfers are resumable, rerunning is safe".into())
            }
            Error::SmallObjectCap { .. } => {
                Some("the server sent a small object beyond its protocol bound: check the server health, or a proxy answering for it".into())
            }
            Error::ServerBody { .. } => {
                Some("the server answered with a body the protocol does not define: check the server health, or a proxy answering for it".into())
            }
        };
        hint.map(|hint| match self.server_body() {
            Some(detail) => {
                // The body is server-controlled: escape control characters so a
                // hostile 401/403 cannot inject ANSI or OSC into a terminal.
                let detail: String = detail.chars().flat_map(char::escape_debug).collect();
                if detail.contains("End User License Agreement") {
                    format!("{hint}. The server requires the EULA accepted first: run `nxr service eula <server-root> --accept`")
                } else {
                    format!("{hint}. server says: {detail}")
                }
            }
            None => hint,
        })
    }

    /// The server's own words, when the response that produced the error carried a short 4xx/5xx body (spec §5).
    /// A transport or io detail is the local reason, never the server's body, so it does not participate.
    fn server_body(&self) -> Option<&str> {
        match self {
            Error::Auth { detail, .. } | Error::ReadOnly { detail, .. } => detail.as_deref(),
            _ => None,
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

    /// Wraps a filesystem error at `path`: downloads surface it as exit 1 data.
    pub fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        Error::Io {
            path: path.display().to_string(),
            detail: source.to_string(),
        }
    }
}

/// Symmetric diff refusal (spec §5.2).
/// Always exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Verdict {
    #[error("mismatch: {}: {detail}", name.escape_debug())]
    Mismatch { name: String, detail: String },
    #[error("missing: {}: exist nowhere", fmt_names(names))]
    Missing { names: Vec<String> },
}

fn fmt_names(names: &[String]) -> String {
    names.join(", ")
}

impl From<Verdict> for Error {
    fn from(v: Verdict) -> Self {
        match v {
            Verdict::Mismatch { name, detail } => Error::Mismatch { name, detail },
            Verdict::Missing { names } => Error::Missing { names },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Server-controlled names must not smuggle terminal escapes into stderr (ANSI/OSC injection): the display renders them escaped.
    #[test]
    fn hostile_server_body_is_escaped_in_the_hint() {
        let e = Error::Auth {
            url: "http://h/x".to_owned(),
            reason: "HTTP 401".to_owned(),
            detail: Some("\u{1b}]0;pwn\u{7}".to_owned()),
        };
        let hint = e.hint().expect("auth carries a hint");
        assert!(!hint.contains('\u{1b}'), "raw ESC leaked: {hint:?}");
    }

    #[test]
    fn unsafe_name_display_is_escaped() {
        let e = Error::UnsafeName {
            name: "\u{1b}[31mevil\u{1b}[0m".to_owned(),
            reason: "grammar".to_owned(),
        };
        let text = e.to_string();
        assert!(!text.contains('\u{1b}'), "raw ESC leaked: {text:?}");
        assert!(
            text.contains("\\u{1b}"),
            "the escape renders visibly: {text:?}"
        );
    }

    /// The crate docs promise a hint for every error: pin it for all variants.
    #[test]
    fn every_error_class_carries_a_hint() {
        let classes = [
            Error::Mismatch {
                name: "a.zip".into(),
                detail: "digest".into(),
            },
            Error::Incomplete {
                names: vec!["a.zip".into()],
            },
            Error::UnsafeName {
                name: "/abs".into(),
                reason: "absolute".into(),
            },
            Error::Missing {
                names: vec!["a.zip".into()],
            },
            Error::Enumerate {
                url: "http://x/".into(),
                reason: "no enumeration source".into(),
            },
            Error::Auth {
                url: "http://x/".into(),
                reason: "401".into(),
                detail: None,
            },
            Error::ReadOnly {
                url: "http://x/".into(),
                status: 403,
                detail: None,
            },
            Error::Transport {
                url: "http://x/".into(),
                detail: "reset".into(),
            },
            Error::Http {
                status: 404,
                url: "http://x/".into(),
            },
            Error::Http {
                status: 503,
                url: "http://x/".into(),
            },
            Error::ServiceMissing {
                url: "http://x/".into(),
                root: "http://x".into(),
            },
            Error::SearchRepoMissing {
                url: "http://x/".into(),
            },
            Error::Io {
                path: "a.zip".into(),
                detail: "disk".into(),
            },
            Error::Misuse("bad flag".into()),
            Error::RangeIgnored {
                url: "http://x/".into(),
            },
        ];
        for e in &classes {
            let hint = e.hint().unwrap_or_else(|| panic!("no hint for {e}"));
            assert!(!hint.trim().is_empty(), "empty hint for {e}");
        }
    }

    /// The hint text is public surface, quoted verbatim in the errors reference: pin the exact string.
    #[test]
    fn search_repo_missing_hint_is_pinned_verbatim() {
        let e = Error::SearchRepoMissing {
            url: "http://x/service/rest/v1/search/assets?repository=raw-ghost".into(),
        };
        assert_eq!(
            e.hint().as_deref(),
            Some("the repository is missing on the server or is not a raw repository")
        );
    }

    /// The server's body rides the hint verbatim after `server says: ` (spec §5): the EULA gate names itself this way.
    #[test]
    fn server_body_is_appended_to_the_hint() {
        let e = Error::Auth {
            url: "http://x/".into(),
            reason: "HTTP 403 Forbidden; pass -u user:pass or export NXR_AUTH (base64 user:pass)"
                .into(),
            detail: Some("You must accept the End User License Agreement".into()),
        };
        assert_eq!(
            e.hint().as_deref(),
            Some(
                "pass -u user:pass or export NXR_AUTH (base64 user:pass). The server requires the EULA accepted first: run `nxr service eula <server-root> --accept`"
            )
        );
    }
}
