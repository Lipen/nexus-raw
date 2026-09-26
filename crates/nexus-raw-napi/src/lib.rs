//! Node bindings for nexus-raw: the `nxr` CLI surface as promise-returning functions.
//!
//! The addon is built on napi-rs 3 (the current stable major, `napi` 3.x with
//! the `@napi-rs/cli` 3.x build tool): async exports run on napi's built-in
//! multi-threaded tokio runtime (`async` feature, drivers all enabled), and
//! events cross the boundary as parsed JSON (`serde-json` feature).
//!
//! Every command is one self-sufficient call, exactly like the CLI: the base
//! URL is in argv, credentials come from `auth` or the env fallback
//! (`NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`), no config file.
//! A promise resolves to the command's result and rejects with an `Error`
//! whose `exitCode` and `hint` mirror `Error::exit_code` and `Error::hint`
//! of the core; the JS entry (`index.js`) promotes them from the rejection
//! message the raw addon produces.
//!
//! Platform status: `linux-x86_64-gnu` is the wired prebuilt target.
//! macOS and Windows triples are known-good for the core but wait for
//! release runners; no prebuilt packages exist for them yet.

mod mapping;

use std::path::{Path, PathBuf};

use napi::bindgen_prelude::{Buffer, Either};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi::{Error, Result, Status};
use napi_derive::napi;
use nexus_raw_core::model::digest;
use nexus_raw_core::{
    ArtifactName, ChannelOutcome, Enumeration, GetOutcome, Manifest, Mode, Nxr, ShaSource, Summary,
};
use tokio::sync::mpsc;

use crate::mapping::CommonOpts;

/// The parsed JSON event the `onEvent` callback receives.
type EventCallback = ThreadsafeFunction<serde_json::Value, (), serde_json::Value, Status, false>;

// ---- Node-facing option objects -----------------------------------------

/// Credentials for the `Authorization` header, curl style.
///
/// When `auth` is absent the env fallback applies: `NXR_AUTH`
/// (base64 `user:pass`), then `NXR_USERNAME` + `NXR_PASSWORD`.
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
    /// Explicit credentials; they win over the env fallback.
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

/// Extract the mapping common options and the event callback from the
/// common option fields every command opts object carries.
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
    /// Explicit credentials; they win over the env fallback.
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
    /// Output file; without it the body streams to the process stdout, like the CLI.
    pub out: Option<String>,
    /// Resume from an existing `<out>.part` through a Range request.
    pub cont: Option<bool>,
}

/// `put` options.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrPutOpts {
    /// Explicit credentials; they win over the env fallback.
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
    /// Explicit credentials; they win over the env fallback.
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
    /// Explicit credentials; they win over the env fallback.
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

/// `verify` options: restrict what is checked.
#[derive(Default)]
#[napi(object, object_to_js = false)]
pub struct NxrVerifyOpts {
    /// Explicit credentials; they win over the env fallback.
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
    pub failed: Vec<String>,
}

impl From<&Summary> for NxrSummary {
    fn from(s: &Summary) -> Self {
        Self {
            uploaded: u32::try_from(s.uploaded).unwrap_or(u32::MAX),
            downloaded: u32::try_from(s.downloaded).unwrap_or(u32::MAX),
            skipped: u32::try_from(s.skipped).unwrap_or(u32::MAX),
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

// ---- helpers ---------------------------------------------------------------

/// Map a core error onto the rejection message: the CLI-style text with the
/// exit line and the hint appended, which `index.js` promotes to `.exitCode`
/// and `.hint` on the rejected Error.
fn js_error(e: nexus_raw_core::Error) -> Error {
    let payload = mapping::error_payload(&e);
    Error::new(Status::GenericFailure, payload.message)
}

/// Drain the core event stream into the JS callback.
///
/// The pump ends when the facade drops and the channel closes, so the
/// promise resolves only after every event has been handed to JS.
fn spawn_pump(
    mut rx: mpsc::UnboundedReceiver<nexus_raw_core::Event>,
    on_event: Option<EventCallback>,
) -> Option<tokio::task::JoinHandle<()>> {
    on_event.map(|tsfn| {
        napi::bindgen_prelude::spawn(async move {
            while let Some(event) = rx.recv().await {
                tsfn.call(
                    mapping::event_to_json(&event),
                    ThreadsafeFunctionCallMode::NonBlocking,
                );
            }
        })
    })
}

/// Wait for the pump to drain before the promise settles.
async fn finish_pump(pump: Option<tokio::task::JoinHandle<()>>) -> Result<()> {
    match pump {
        None => Ok(()),
        Some(handle) => handle
            .await
            .map_err(|e| Error::new(Status::GenericFailure, format!("event pump failed: {e}"))),
    }
}

/// Resolve a `manifest` spec: `-` for stdin, http(s) URLs through the
/// server, everything else as a local file — the CLI rules.
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
        Some(v) if v.is_empty() => Ok(None),
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
    let outcome: GetOutcome = outcome.map_err(js_error)?;
    finish_pump(pump).await?;
    Ok(NxrGetResult {
        size: outcome.size as f64,
        sha256: outcome.digest.map(|d| d.as_str().to_owned()),
        resumed_from: outcome.resumed_from as f64,
    })
}

/// PUT a file path or an exact byte body, optionally with its sha-sibling.
///
/// A string source is a file path (like the CLI `-f`); a Buffer is the
/// exact bytes, staged through a temporary file.
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
    let (size, digest) = uploaded.map_err(js_error)?;
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
    let info = info.map_err(js_error)?;
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
    let d = d.map_err(js_error)?;
    finish_pump(pump).await?;
    Ok(d.as_str().to_owned())
}

// ---- L1 transfer -----------------------------------------------------------

/// Upload a local directory: verified, parallel, marker-perfect.
///
/// With `dryRun` the promise resolves to the plan instead of a summary and
/// nothing transfers, like `up --dry-run`.
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
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => Some(load_manifest(&nxr, spec).await.map_err(js_error)?.names),
        None => parse_names(o.names)?,
    };
    let claim = match &o.claim_first {
        Some(raw) => Some(ArtifactName::parse(raw).map_err(js_error)?),
        None => None,
    };
    let gen_markers = !o.no_sha.unwrap_or(false);
    if o.dry_run.unwrap_or(false) {
        let scanned = match names {
            Some(n) => n,
            None => nxr.scan(Path::new(&src_dir)).map_err(js_error)?,
        };
        let actions = nxr
            .diff(Path::new(&src_dir), scanned, Mode::Up, gen_markers)
            .await
            .map_err(js_error)?;
        return Ok(Either::B(NxrPlan {
            actions: actions.iter().map(NxrPlanAction::from).collect(),
        }));
    }
    let pump = spawn_pump(rx, on_event);
    let summary = nxr
        .up(Path::new(&src_dir), names, gen_markers, claim, None)
        .await;
    drop(nxr);
    let summary = summary.map_err(js_error)?;
    finish_pump(pump).await?;
    Ok(Either::A(NxrSummary::from(&summary)))
}

/// Download a remote directory into a local one.
///
/// The enumeration source is mandatory: `ls`, `manifest`, explicit `names`,
/// or the conventional `manifest.json` at the directory URL — otherwise the
/// promise rejects like the CLI.
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
    let enum_src = if o.ls.unwrap_or(false) {
        Enumeration::Search
    } else if let Some(spec) = &o.manifest {
        Enumeration::Manifest(load_manifest(&nxr, spec).await.map_err(js_error)?)
    } else {
        match parse_names(o.names)? {
            Some(v) => Enumeration::Names(v),
            None => match nxr.manifest_at_base().await.map_err(js_error)? {
                Some(m) => Enumeration::Manifest(m),
                None => {
                    return Err(js_error(nexus_raw_core::Error::Enumerate {
                        url: nxr.base().to_owned(),
                        reason: "no manifest.json on the server and no manifest/names/ls given"
                            .into(),
                    }))
                }
            },
        }
    };
    let pump = spawn_pump(rx, on_event);
    let summary = nxr
        .down(
            Path::new(&dst_dir),
            enum_src,
            o.fresh.unwrap_or(false),
            None,
        )
        .await;
    drop(nxr);
    let summary = summary.map_err(js_error)?;
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
    let names: Option<Vec<ArtifactName>> = match &o.manifest {
        Some(spec) => Some(load_manifest(&nxr, spec).await.map_err(js_error)?.names),
        None => parse_names(o.names)?,
    };
    let pump = spawn_pump(rx, on_event);
    let summary = nxr.verify(Path::new(&dir), names).await;
    drop(nxr);
    let summary = summary.map_err(js_error)?;
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
    let token = token.map_err(js_error)?;
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
    let outcome = outcome.map_err(js_error)?;
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
