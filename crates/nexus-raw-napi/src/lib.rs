//! Node bindings for nexus-raw: the `nxr` CLI surface as promise-returning functions.
//!
//! Built on napi-rs 3: async exports run on napi's built-in multi-threaded tokio runtime (the `async` feature with all drivers enabled).
//! Events cross the boundary as parsed JSON (the `serde-json` feature).
//!
//! Every command is one self-sufficient call, exactly like the CLI: the base URL is in argv, credentials come from `auth` or the env fallback (`NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`), no config file.
//! A promise resolves to the command's result and rejects with an `Error` whose `exitCode` and `hint` mirror `Error::exit_code` and `Error::hint` of the core.
//! The JS entry (`index.js`) promotes them from the rejection message the raw addon produces.
//!
//! Platform status: `linux-x86_64-gnu`, `darwin-x64` and `darwin-arm64` ship as prebuilt packages.
//! The win32 addon waits on an npm ticket; there the loader falls back to a local build.

mod mapping;

use std::path::{Path, PathBuf};

use napi::bindgen_prelude::{Buffer, Either};
use napi::threadsafe_function::ThreadsafeFunction;
use napi::{Error, Result, Status};
use napi_derive::napi;
use nexus_raw_core::model::digest;
use nexus_raw_core::{
    ArtifactName, ChannelOutcome, ClearOutcome, Enumeration, GetOutcome, Manifest, Mode, Nxr,
    RmAction, ShaSource, Summary,
};
use tokio::sync::mpsc;

use crate::mapping::CommonOpts;

/// The parsed JSON event the `onEvent` callback receives.
type EventCallback = ThreadsafeFunction<serde_json::Value, (), serde_json::Value, Status, false>;

/// The drain handle of a started event pump.
/// The pump ends with the first callback failure, if one happens.
type Pump = tokio::task::JoinHandle<Result<()>>;

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
fn split_common(
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
    /// Resolve to the plan without transferring anything (`up --dry-run`).
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

// ---- Node-facing result objects -------------------------------------------

/// The final summary of a transfer or verification.
#[napi(object)]
pub struct NxrSummary {
    pub uploaded: u32,
    pub downloaded: u32,
    pub skipped: u32,
    /// Names the call deleted (`rm`); transfers never delete and stay at 0.
    pub removed: u32,
    pub failed: Vec<String>,
}

impl From<&Summary> for NxrSummary {
    fn from(s: &Summary) -> Self {
        Self {
            uploaded: u32::try_from(s.uploaded).unwrap_or(u32::MAX),
            downloaded: u32::try_from(s.downloaded).unwrap_or(u32::MAX),
            skipped: u32::try_from(s.skipped).unwrap_or(u32::MAX),
            removed: u32::try_from(s.removed).unwrap_or(u32::MAX),
            failed: s.failed.clone(),
        }
    }
}

/// One dry-run action, shaped like the CLI `--json` dry-run lines.
#[napi(object)]
pub struct NxrPlanAction {
    /// `skip`, `upload` or `download`.
    pub action: String,
    pub name: String,
    /// Byte size when the action transfers bytes.
    pub size: Option<f64>,
}

/// The plan a dry run resolves to.
#[napi(object)]
pub struct NxrPlan {
    pub actions: Vec<NxrPlanAction>,
}

/// One planned deletion, shaped like the CLI `rm --dry-run` lines.
#[napi(object)]
pub struct NxrRmPlanAction {
    /// `rm` when the remote copy exists and would be deleted, `missing` when it is already absent.
    pub action: String,
    pub name: String,
    /// Byte size when the remote copy exists.
    pub size: Option<f64>,
}

impl From<&RmAction> for NxrRmPlanAction {
    fn from(a: &RmAction) -> Self {
        match a {
            RmAction::Remove { name, size } => Self {
                action: "rm".into(),
                name: name.to_string(),
                size: size.map(|s| s as f64),
            },
            RmAction::Missing { name } => Self {
                action: "missing".into(),
                name: name.to_string(),
                size: None,
            },
        }
    }
}

/// The plan an `rm` dry run resolves to.
#[napi(object)]
pub struct NxrRmPlan {
    pub actions: Vec<NxrRmPlanAction>,
}

impl From<&nexus_raw_core::Action> for NxrPlanAction {
    fn from(a: &nexus_raw_core::Action) -> Self {
        match a {
            nexus_raw_core::Action::Skip { name, .. } => Self {
                action: "skip".into(),
                name: name.to_string(),
                size: None,
            },
            nexus_raw_core::Action::Upload { name, size, .. } => Self {
                action: "upload".into(),
                name: name.to_string(),
                size: Some(*size as f64),
            },
            nexus_raw_core::Action::Download { name, size, .. } => Self {
                action: "download".into(),
                name: name.to_string(),
                size: size.map(|s| s as f64),
            },
        }
    }
}

/// The `get` result.
#[napi(object)]
pub struct NxrGetResult {
    /// Final size in bytes (stdout mode: bytes written).
    pub size: f64,
    /// The digest when a file was produced (stdout streaming does not hash).
    pub sha256: Option<String>,
    /// The offset the transfer resumed from (0 for a fresh download).
    pub resumed_from: f64,
}

/// The `put` result.
#[napi(object)]
pub struct NxrPutResult {
    pub size: f64,
    /// The uploaded digest when the sha-sibling was requested.
    pub sha256: Option<String>,
}

/// The `head` result.
#[napi(object)]
pub struct NxrHeadResult {
    pub status: u32,
    pub size: Option<f64>,
    pub content_type: Option<String>,
}

/// The `channelSet` result.
#[napi(object)]
pub struct NxrChannelSetResult {
    /// False when the forward-only guard kept the current token.
    pub written: bool,
    /// The previous token, when one was readable.
    pub from: Option<String>,
    /// The kept token, when the write was skipped.
    pub current: Option<String>,
}

/// The `pointClear` result, the CLI `--json` shape.
#[napi(object)]
pub struct NxrPointClearResult {
    /// `cleared` when the pointer existed and is deleted, `absent` when it was already gone.
    pub outcome: String,
}

// ---- helpers ---------------------------------------------------------------

/// Map a core error onto the rejection message: the CLI-style text with the exit line and the hint appended.
/// `index.js` promotes them to `.exitCode` and `.hint` on the rejected Error.
fn js_error(e: nexus_raw_core::Error) -> Error {
    let payload = mapping::error_payload(&e);
    js_error_message(e.to_string(), &payload)
}

/// Build the rejection from a text plus the trailing machine lines of a payload.
fn js_error_message(text: String, payload: &mapping::ErrorPayload) -> Error {
    let mut message = text;
    message.push_str(&format!("\nnxr:exit {}", payload.exit_code));
    if let Some(hint) = &payload.hint {
        message.push_str("\nhint: ");
        message.push_str(hint);
    }
    Error::new(Status::GenericFailure, message)
}

/// Drain the core event stream into the JS callback.
///
/// Every event is awaited through `call_async_catch`: a throw inside the callback ends the pump with that error instead of becoming a global uncaught exception.
/// The pump ends when the facade drops and the channel closes, so the promise settles only after every event has been handed to JS.
fn spawn_pump(
    mut rx: mpsc::UnboundedReceiver<nexus_raw_core::Event>,
    on_event: Option<EventCallback>,
) -> Option<Pump> {
    on_event.map(|tsfn| {
        // tokio::spawn, not the napi re-export: the re-export disappears under the noop feature that unit tests need for linking.
        // Inside a napi async fn the current runtime is napi's own tokio RT, so both calls land on the same workers (napi-3.13 tokio_runtime.rs: `spawn` is `RT.spawn`).
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Err(callback) = tsfn.call_async_catch(mapping::event_to_json(&event)).await {
                    return Err(Error::new(
                        Status::GenericFailure,
                        format!("the onEvent callback failed: {callback}"),
                    ));
                }
            }
            Ok(())
        })
    })
}

/// Wait for the pump to drain before the promise settles.
///
/// A callback failure rejects the command promise even when the command itself succeeded.
async fn finish_pump(pump: Option<Pump>) -> Result<()> {
    match pump {
        None => Ok(()),
        Some(handle) => match handle.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(callback)) => Err(callback),
            Err(join) => Err(Error::new(
                Status::GenericFailure,
                format!("event pump failed: {join}"),
            )),
        },
    }
}

/// Drain the event pump, then map `error` for JS.
/// The caller must see the events that led to the failure before the promise rejects.
/// The command's exit code and hint stay untouched: a callback failure only adds a note above the machine lines.
async fn finish_pump_err(pump: Option<Pump>, error: nexus_raw_core::Error) -> Error {
    let payload = mapping::error_payload(&error);
    let mut text = error.to_string();
    if let Some(handle) = pump {
        if let Ok(Err(callback)) = handle.await {
            text.push_str("\nonEvent callback also failed: ");
            text.push_str(&callback.reason);
        }
    }
    js_error_message(text, &payload)
}

/// Resolve the enumeration source the way the CLI does: `ls`, a `manifest` spec, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// The caller must drop the facade before draining the pump on the error path.
async fn resolve_enumeration(
    nxr: &Nxr,
    manifest: &Option<String>,
    parsed_names: Option<Vec<ArtifactName>>,
    ls: bool,
    no_manifest_reason: &'static str,
) -> std::result::Result<Enumeration, nexus_raw_core::Error> {
    if ls {
        return Ok(Enumeration::Search);
    }
    if let Some(spec) = manifest {
        return load_manifest(nxr, spec).await.map(Enumeration::Manifest);
    }
    if let Some(v) = parsed_names {
        return Ok(Enumeration::Names(v));
    }
    match nxr.manifest_at_base().await {
        Ok(Some(m)) => Ok(Enumeration::Manifest(m)),
        Ok(None) => Err(nexus_raw_core::Error::Enumerate {
            url: nxr.base().to_owned(),
            reason: no_manifest_reason.to_owned(),
        }),
        Err(e) => Err(e),
    }
}

/// Resolve a `manifest` spec: `-` for stdin, http(s) URLs through the server, everything else as a local file — the CLI rules.
async fn load_manifest(
    nxr: &Nxr,
    spec: &str,
) -> std::result::Result<Manifest, nexus_raw_core::Error> {
    match mapping::classify_manifest_spec(spec) {
        mapping::ManifestSpec::Stdin => Manifest::from_stdin(),
        mapping::ManifestSpec::Url(url) => nxr.manifest_from(&url).await,
        mapping::ManifestSpec::File(path) => Manifest::from_file(Path::new(&path)),
    }
}

/// Parse explicit names through the grammar (exit 2 on bad names).
fn parse_names(names: Option<Vec<String>>) -> Result<Option<Vec<ArtifactName>>> {
    match names {
        None => Ok(None),
        // An explicit empty list restricts nothing: that is a caller bug, not a whole-catalog request.
        Some(v) if v.is_empty() => Err(js_error(nexus_raw_core::Error::Misuse(
            "names: an empty list would transfer the whole catalog; omit the \
             option for that, or name at least one artifact"
                .to_owned(),
        ))),
        Some(v) => v
            .iter()
            .map(|s| ArtifactName::parse(s).map_err(js_error))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(Some),
    }
}

// ---- L0 primitives ---------------------------------------------------------

/// GET a URL to a file or the process stdout.
#[napi]
pub async fn get(url: String, opts: Option<NxrGetOpts>) -> Result<NxrGetResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let outcome = nxr
        .get(&url, o.out.map(PathBuf::from), o.cont.unwrap_or(false))
        .await;
    drop(nxr);
    let outcome: GetOutcome = match outcome {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrGetResult {
        size: outcome.size as f64,
        sha256: outcome.digest.map(|d| d.as_str().to_owned()),
        resumed_from: outcome.resumed_from as f64,
    })
}

/// PUT a file path or an exact byte body, optionally with its sha-sibling.
///
/// A string source is a file path (like the CLI `-f`).
/// A Buffer is the exact bytes, staged through a temporary file.
#[napi]
pub async fn put(
    url: String,
    source: Either<String, Buffer>,
    opts: Option<NxrPutOpts>,
) -> Result<NxrPutResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    // A byte body is staged through a temp file that lives until the PUT ends.
    let staged = match &source {
        Either::A(_) => None,
        Either::B(body) => {
            let mut file = tempfile::NamedTempFile::new().map_err(napi::Error::from)?;
            std::io::Write::write_all(file.as_file_mut(), body.as_ref())
                .map_err(napi::Error::from)?;
            Some(file)
        }
    };
    let path = match (&source, &staged) {
        (Either::A(path), _) => PathBuf::from(path),
        (Either::B(_), Some(file)) => file.path().to_owned(),
        (Either::B(_), None) => unreachable!("a byte body always stages a temp file"),
    };
    let uploaded = nxr.put(&url, &path, o.sha.unwrap_or(false)).await;
    drop(nxr);
    let (size, digest) = match uploaded {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrPutResult {
        size: size as f64,
        sha256: digest.map(|d| d.as_str().to_owned()),
    })
}

/// HEAD a URL: status, size, content type.
#[napi]
pub async fn head(url: String, opts: Option<NxrCommonOpts>) -> Result<NxrHeadResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let info = nxr.head(&url).await;
    drop(nxr);
    let info = match info {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrHeadResult {
        status: u32::from(info.status),
        size: info.size.map(|s| s as f64),
        content_type: info.content_type,
    })
}

/// The sha256 of a local file or an http(s) URL, as 64 lowercase hex chars.
#[napi]
pub async fn sha(target: String, opts: Option<NxrCommonOpts>) -> Result<String> {
    // A local file needs no HTTP client at all.
    if !mapping::is_url_target(&target) {
        let path = PathBuf::from(&target);
        return digest::sha256_file(&path)
            .map(|d| d.as_str().to_owned())
            .map_err(|e| {
                js_error(nexus_raw_core::Error::Misuse(format!(
                    "{}: {e}",
                    path.display()
                )))
            });
    }
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&target, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let d = nxr.sha(ShaSource::Url(target.clone())).await;
    drop(nxr);
    let d = match d {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(d.as_str().to_owned())
}

// ---- L1 transfer -----------------------------------------------------------

/// Upload a local directory.
///
/// With `dryRun` the promise resolves to the plan instead of a summary and nothing transfers, like `up --dry-run`.
#[napi]
pub async fn up(
    src_dir: String,
    dst_url: String,
    opts: Option<NxrUpOpts>,
) -> Result<Either<NxrSummary, NxrPlan>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&dst_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let claim = match &o.claim_first {
        Some(raw) => Some(ArtifactName::parse(raw).map_err(js_error)?),
        None => None,
    };
    let gen_markers = !o.no_sha.unwrap_or(false);
    // The pump starts before any network call: events fired during the manifest fetch belong to JS as much as the later ones.
    let pump = spawn_pump(rx, on_event);
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => match load_manifest(&nxr, spec).await {
            Ok(m) => Some(m.names),
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        },
        None => parsed_names,
    };
    if o.dry_run.unwrap_or(false) {
        let scanned = match names {
            Some(n) => n,
            None => match nxr.scan(Path::new(&src_dir)) {
                Ok(n) => n,
                Err(e) => {
                    drop(nxr);
                    return Err(finish_pump_err(pump, e).await);
                }
            },
        };
        let actions = match nxr
            .diff(Path::new(&src_dir), scanned, Mode::Up, gen_markers)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        };
        // The plan promise settles only after the events did, and the events end only when the facade's sender is gone.
        drop(nxr);
        finish_pump(pump).await?;
        return Ok(Either::B(NxrPlan {
            actions: actions.iter().map(NxrPlanAction::from).collect(),
        }));
    }
    let summary = nxr.up(Path::new(&src_dir), names, gen_markers, claim).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(Either::A(NxrSummary::from(&summary)))
}

/// Download a remote directory into a local one.
///
/// The enumeration source is mandatory: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// Otherwise the promise rejects like the CLI.
#[napi]
pub async fn down(
    src_url: String,
    dst_dir: String,
    opts: Option<NxrDownOpts>,
) -> Result<NxrSummary> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&src_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    // The pump starts before any network call: events fired during the manifest fetch belong to JS as much as the later ones.
    // Every early return from here drops the facade before draining the pump — the pump ends only when the facade's sender is gone.
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json on the server and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    let summary = nxr
        .down(Path::new(&dst_dir), enum_src, o.fresh.unwrap_or(false))
        .await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}

/// Delete the enumerated names from the remote directory.
///
/// The enumeration source is mandatory, like `down`: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// Divergence is never checked: `rm` deletes names, not content.
/// With `dryRun` the promise resolves to the plan instead of a summary and nothing is deleted, like `rm --dry-run`.
#[napi]
pub async fn rm(src_url: String, opts: Option<NxrRmOpts>) -> Result<Either<NxrSummary, NxrRmPlan>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&src_url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json on the server and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    if o.dry_run.unwrap_or(false) {
        let plan = nxr.rm_plan(enum_src).await;
        drop(nxr);
        let actions = match plan {
            Ok(a) => a,
            Err(e) => return Err(finish_pump_err(pump, e).await),
        };
        finish_pump(pump).await?;
        return Ok(Either::B(NxrRmPlan {
            actions: actions.iter().map(NxrRmPlanAction::from).collect(),
        }));
    }
    let summary = nxr.rm(enum_src).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(Either::A(NxrSummary::from(&summary)))
}

/// Pour enumerated names from a source repository into a destination one.
///
/// Two facades on one event stream: enumeration and bytes come from the source, the diff and the writes follow the destination.
/// The enumeration source is mandatory and lives at the source: `ls`, `manifest`, explicit `names`, or the conventional `manifest.json` there.
#[napi]
pub async fn mirror(
    src_url: String,
    dst_url: String,
    opts: Option<NxrMirrorOpts>,
) -> Result<NxrSummary> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let side_auth = |side: &Option<NxrAuth>| match side {
        Some(a) => CommonOpts {
            auth_user: Some(a.user.clone()),
            auth_pass: Some(a.pass.clone()),
            ..common.clone()
        },
        None => common.clone(),
    };
    let src_cfg = mapping::build_config(&src_url, &side_auth(&o.src_auth)).map_err(js_error)?;
    let dst_cfg = mapping::build_config(&dst_url, &side_auth(&o.dst_auth)).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    // Both facades build before anything moves: a bad destination URL is misuse, not a half-poured version.
    let src_nxr = Nxr::new(src_cfg, tx.clone()).map_err(js_error)?;
    let dst_nxr = Nxr::new(dst_cfg, tx).map_err(js_error)?;
    // Pure parsing first: no events can precede the pump.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let enum_src = match resolve_enumeration(
        &src_nxr,
        &o.manifest,
        parsed_names,
        o.ls.unwrap_or(false),
        "no manifest.json at the source and no manifest/names/ls given",
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            drop(src_nxr);
            drop(dst_nxr);
            return Err(finish_pump_err(pump, e).await);
        }
    };
    let summary = src_nxr.mirror(&dst_nxr, enum_src).await;
    // Both facades must die before the pump drains: each holds a sender clone.
    drop(src_nxr);
    drop(dst_nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}

/// Check local bytes, markers and digests. No network.
#[napi]
pub async fn verify(dir: String, opts: Option<NxrVerifyOpts>) -> Result<NxrSummary> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    // verify is local-only: the base is a placeholder, like the CLI.
    let cfg = mapping::build_config("http://localhost/", &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    // Pure parsing first: the pump starts before any manifest fetch.
    let parsed_names: Option<Vec<ArtifactName>> = parse_names(o.names)?;
    let pump = spawn_pump(rx, on_event);
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => match load_manifest(&nxr, spec).await {
            Ok(m) => Some(m.names),
            Err(e) => {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        },
        None => parsed_names,
    };
    let summary = nxr.verify(Path::new(&dir), names).await;
    drop(nxr);
    let summary = match summary {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrSummary::from(&summary))
}

// ---- L2 layout -------------------------------------------------------------

/// Read a channel ref: the token, or null when the channel is unset.
#[napi]
pub async fn channel_get(url: String, opts: Option<NxrCommonOpts>) -> Result<Option<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let token = nxr.channel_get(&url).await;
    drop(nxr);
    let token = match token {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(token)
}

/// Write a channel token with the optional forward-only guard.
#[napi]
pub async fn channel_set(
    url: String,
    token: String,
    if_forward: bool,
    opts: Option<NxrCommonOpts>,
) -> Result<NxrChannelSetResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let outcome = nxr.channel_set(&url, &token, if_forward).await;
    drop(nxr);
    let outcome = match outcome {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(match outcome {
        ChannelOutcome::Written { from } => NxrChannelSetResult {
            written: true,
            from,
            current: None,
        },
        ChannelOutcome::Skipped { current } => NxrChannelSetResult {
            written: false,
            from: None,
            current: Some(current),
        },
    })
}

/// DELETE a pointer file: absence is a normal outcome, like the CLI `point --clear`.
#[napi]
pub async fn point_clear(url: String, opts: Option<NxrCommonOpts>) -> Result<NxrPointClearResult> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let outcome = nxr.point_clear(&url).await;
    drop(nxr);
    let outcome = match outcome {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(NxrPointClearResult {
        outcome: match outcome {
            ClearOutcome::Cleared => "cleared".into(),
            ClearOutcome::Absent => "absent".into(),
        },
    })
}

/// List the asset names the server search API reports for this directory, like `ls --assets`.
#[napi]
pub async fn ls_assets(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let names = nxr.ls_assets().await;
    drop(nxr);
    let names = match names {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(names.iter().map(|n| n.as_str().to_owned()).collect())
}

/// List the version tokens the server search API reports for this directory, like `ls`.
#[napi]
pub async fn ls_versions(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<String>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let versions = nxr.ls_versions().await;
    drop(nxr);
    let versions = match versions {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(versions)
}

#[cfg(test)]
mod tests {
    use super::parse_names;

    #[test]
    fn empty_names_list_is_misuse_not_whole_catalog() {
        assert!(parse_names(Some(vec![])).is_err());
        assert!(parse_names(None).unwrap().is_none());
        assert_eq!(
            parse_names(Some(vec!["a.zip".into()]))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
    }
}

/// One repository of a Nexus server, as the service REST API reports it.
#[napi(object)]
pub struct NxrRepoInfo {
    /// Repository name.
    pub name: String,
    /// Repository format (`raw`, `maven2`, ...), spelled as the server spells it.
    pub format: String,
    /// Repository kind (`hosted`, `proxy`, `group`).
    pub kind: String,
    /// Repository URL.
    pub url: String,
}

/// List the repositories of the server behind `url`: the service REST API (the management surface).
/// The URL may be the server root or any repository URL: both root to the same server.
/// Storage invariants never touch this endpoint; it exists for humans and panels.
#[napi]
pub async fn service_repos(url: String, opts: Option<NxrCommonOpts>) -> Result<Vec<NxrRepoInfo>> {
    let o = opts.unwrap_or_default();
    let (common, on_event) = split_common(
        o.auth,
        o.workers,
        o.retry,
        o.connect_timeout_ms,
        o.stall_ms,
        o.tls_insecure,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    let repos = nxr.service_repos().await;
    drop(nxr);
    let repos = match repos {
        Ok(v) => v,
        Err(e) => return Err(finish_pump_err(pump, e).await),
    };
    finish_pump(pump).await?;
    Ok(repos
        .into_iter()
        .map(|r| NxrRepoInfo {
            name: r.name,
            format: r.format,
            kind: r.kind,
            url: r.url,
        })
        .collect())
}
