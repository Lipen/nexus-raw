use napi_derive::napi;

use crate::mapping::CommonOpts;
use crate::pump::EventCallback;

// ---- Node-facing option objects -----------------------------------------

/// Credentials for the `Authorization` header, curl style.
///
/// When `auth` is absent the env fallback applies: `NXR_AUTH` (base64 `user:pass`), then `NXR_USERNAME` + `NXR_PASSWORD`.
#[derive(Default)]
#[napi(object)]
pub struct NxrAuth {
    pub user: String,
    pub pass: String,
}

/// The options every command shares: transport tuning, credentials, events.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrCommonOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
}

/// Extract the mapping common options and the event callback from the common option fields every command opts object carries.
#[allow(clippy::too_many_arguments)]
pub(crate) fn split_common(
    auth: Option<NxrAuth>,
    workers: Option<u32>,
    retry: Option<u32>,
    connect_timeout_ms: Option<u32>,
    stall_ms: Option<u32>,
    tls_insecure: Option<bool>,
    on_event: Option<EventCallback>,
) -> (CommonOpts, Option<EventCallback>) {
    let (auth_user, auth_pass) = match auth {
        Some(a) => (Some(a.user), Some(a.pass)),
        None => (None, None),
    };
    (
        CommonOpts {
            auth_user,
            auth_pass,
            workers,
            retry,
            connect_timeout_ms,
            stall_ms,
            tls_insecure,
        },
        on_event,
    )
}

/// `get` options: where the bytes go.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrGetOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Output file.
    /// Without it the body streams to the process stdout, like the CLI.
    pub out: Option<String>,
    /// Resume from an existing `<out>.part` through a Range request.
    pub cont: Option<bool>,
}

/// `put` options.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrPutOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Also PUT `<url>`.sha256 with the sha256sum-style marker.
    pub sha: Option<bool>,
}

/// `up` options: what to upload and how.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrUpOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Restrict the transfer to these names: a manifest file, URL or `-` for stdin.
    pub manifest: Option<String>,
    /// Restrict the transfer to these explicit names.
    pub names: Option<Vec<String>>,
    /// Upload this file first, alone, before any other name (claim-first).
    pub claim_first: Option<String>,
    /// Skip marker generation and marker uploads.
    pub no_sha: Option<bool>,
    /// Resolve to the plan without transferring anything (`up --plan`).
    pub dry_run: Option<bool>,
}

/// `down` options: how the remote directory is enumerated.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrDownOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Enumeration source: a manifest file, URL or `-` for stdin.
    pub manifest: Option<String>,
    /// Explicit names to download.
    pub names: Option<Vec<String>>,
    /// Best-effort enumeration through the server search API (`--ls`).
    pub ls: Option<bool>,
    /// Ignore existing part files: every name downloads from zero.
    pub fresh: Option<bool>,
}

/// `rm` options: how the remote directory is enumerated, and whether anything is deleted.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrRmOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Enumeration source: a manifest file, URL or `-` for stdin.
    pub manifest: Option<String>,
    /// Explicit names to delete.
    pub names: Option<Vec<String>>,
    /// Best-effort enumeration through the server search API (`--ls`).
    pub ls: Option<bool>,
    /// Resolve to the plan without deleting anything (`rm --dry-run`).
    pub dry_run: Option<bool>,
}

/// `mirror` options: the enumeration lives at the source, credentials are per side.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrMirrorOpts {
    /// Explicit credentials for both sides when the per-side overrides are absent.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Explicit credentials for the source repository.
    /// They win over `auth` and the env fallback.
    pub src_auth: Option<NxrAuth>,
    /// Explicit credentials for the destination repository.
    /// They win over `auth` and the env fallback.
    pub dst_auth: Option<NxrAuth>,
    /// Enumeration source: a manifest file, URL or `-` for stdin.
    pub manifest: Option<String>,
    /// Explicit names to copy.
    pub names: Option<Vec<String>>,
    /// Best-effort enumeration through the server search API (`--ls`).
    pub ls: Option<bool>,
}

/// `verify` options: restrict what is checked.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrVerifyOpts {
    /// Explicit credentials.
    /// They win over the env fallback.
    pub auth: Option<NxrAuth>,
    /// Parallel artifact transfers, 1..=64, default 8.
    pub workers: Option<u32>,
    /// Attempts per HTTP request, default 4.
    pub retry: Option<u32>,
    /// TCP connect timeout in milliseconds, default 15000.
    pub connect_timeout_ms: Option<u32>,
    /// Fail a transfer when no bytes move for this long, default 30000.
    pub stall_ms: Option<u32>,
    /// Skip TLS certificate verification, default false.
    pub tls_insecure: Option<bool>,
    /// Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
    #[napi(ts_type = "(event: object) => void")]
    pub on_event: Option<EventCallback>,
    /// Check exactly these names: a manifest file, URL or `-` for stdin.
    pub manifest: Option<String>,
    /// Check exactly these explicit names.
    pub names: Option<Vec<String>>,
}
