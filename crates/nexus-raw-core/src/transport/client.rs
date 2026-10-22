//! `NexusClient`: GET/HEAD/PUT over reqwest.
//! Auth, TLS, retries, stall detection, Range resume (spec §5.1).

use std::future::Future;
// The fs-bound methods below are native-only (see the module tail): their
// imports gate with them.
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Mutex;
use std::time::Duration;

// Server error bodies are short by nature: read at most this much into `Error.detail`.
const BODY_DETAIL_CHARS: usize = 120;

use futures_util::StreamExt;
#[cfg(not(target_arch = "wasm32"))]
use sha2::{Digest as _, Sha256};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
use tokio::sync::Semaphore;

use crate::config::Config;
use crate::error::Error;
use crate::events::{Dir, Event, Progress};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::RemoteStatus;
use crate::transport::retry::{is_retryable, AttemptFailure, RetryPolicy};

// Outgoing body chunk size: small enough that a full outbound buffer drains well inside the stall timeout, so channel backpressure means a real stall.
#[cfg(not(target_arch = "wasm32"))]
const UPLOAD_CHUNK: usize = 16 * 1024;
// Depth of the outgoing body channel.
#[cfg(not(target_arch = "wasm32"))]
const UPLOAD_CHANNEL: usize = 4;

// The attempt future carries the reqwest call: on wasm the fetch future is
// `!Send` (JS objects), so the boxed alias keeps the `Send` bound native-only.
#[cfg(not(target_arch = "wasm32"))]
type Attempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttemptFailure>> + Send + 'a>>;
#[cfg(target_arch = "wasm32")]
type Attempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttemptFailure>> + 'a>>;

/// Upper bound for "small" GETs: siblings, channel tokens, search pages.
const SMALL_CAP: u64 = 16 * 1024 * 1024;

/// Open options for files nxr writes under a server-name-derived path (`.part` files, local `.sha256` markers): create/truncate as asked, but never follow a symlink: a predictable name must not become a write into a different file.
///
/// The append arm deliberately lacks `create`: it may only open a part whose prefix was just read.
/// A part that vanished in between must fail loudly. A silently recreated file would yield a truncated "complete" artifact whose digest still matches (the hash covers the prefix that was read, not the bytes on disk).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_options(append: bool) -> tokio::fs::OpenOptions {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if append {
        options.append(true);
    } else {
        options.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        // tokio::fs::OpenOptions mirrors the unix extension natively.
        // Windows has no equivalent flag: the parts live under the user's own
        // temp dir and the name grammar blocks traversal (see the blocking
        // variant below), so the plain options are the accepted answer there.
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
}

/// The blocking counterpart of `write_options` for `spawn_blocking` code.
#[cfg(all(not(target_arch = "wasm32"), unix))]
pub(crate) fn write_options_blocking(append: bool) -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if append {
        options.append(true);
    } else {
        options.create(true).truncate(true);
    }
    options.custom_flags(libc::O_NOFOLLOW);
    options
}

/// Non-unix targets have no O_NOFOLLOW: the plain options keep them building.
/// The name grammar still blocks traversal.
#[cfg(all(not(target_arch = "wasm32"), not(unix)))]
pub(crate) fn write_options_blocking(append: bool) -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    if append {
        options.append(true);
    } else {
        options.create(true).truncate(true);
    }
    options
}

/// What a HEAD saw: the status and the advertised metadata.
/// A 404 is a normal result, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadInfo {
    pub status: u16,
    pub size: Option<u64>,
    pub content_type: Option<String>,
}

/// The result of a DELETE (§5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// The object existed and is gone.
    Deleted,
    /// The server answered 404: the object was already absent.
    Missing,
}

pub struct NexusClient {
    http: reqwest::Client,
    retry: RetryPolicy,
    stall: Duration,
    auth: Option<String>,
    events: Progress,
    workers: Arc<Semaphore>,
    /// The configured worker maximum: the AIMD budget never exceeds it.
    workers_max: usize,
    /// The run's AIMD worker budget, when one is attached (directory transfers).
    /// Set by the facade after construction; primitive-only clients have none,
    /// and their retry outcomes never touch a budget.
    aimd: std::sync::Mutex<Option<Arc<crate::transport::aimd::WorkerLimit>>>,
}

/// Parse a `Content-Range` value: `bytes start-end/total`, where the total may be `*`.
/// Returns the start and end offsets (inclusive) and the total when the server names one.
fn parse_content_range(value: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim().parse().ok()?;
    let total = match total.trim() {
        "*" => None,
        n => Some(n.parse().ok()?),
    };
    Some((start, end, total))
}

/// Parse the total of a `416` answer: `Content-Range: bytes */total`.
fn content_range_total(value: &str) -> Option<u64> {
    value.trim().strip_prefix("bytes */")?.trim().parse().ok()
}

impl NexusClient {
    /// Build the client from `cfg`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when [`Config::validate`] refuses the config or the HTTP client cannot be built.
    pub fn new(cfg: &Config, events: Progress) -> Result<Self, Error> {
        cfg.validate()?;
        // On wasm the browser owns the connect and read timeouts, the TLS trust
        // store and the redirect policy, and fetch has no knobs for them:
        // the native builder options are gated to native targets.
        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let b = reqwest::Client::builder()
                .connect_timeout(cfg.connect_timeout)
                // Control requests (probe, marker PUT, delete, small get/put) have no body
                // loop to watch: this read timeout is their only stall guard.
                .read_timeout(cfg.stall_timeout)
                .redirect(reqwest::redirect::Policy::none());
            if cfg.tls_insecure {
                b.danger_accept_invalid_certs(true)
            } else {
                b
            }
        };
        #[cfg(target_arch = "wasm32")]
        let builder = reqwest::Client::builder();
        let http = builder
            .build()
            .map_err(|e| Error::misuse(format!("http client: {e}")))?;
        Ok(Self {
            http,
            retry: RetryPolicy {
                attempts: cfg.retry_attempts,
                ..RetryPolicy::default()
            },
            stall: cfg.stall_timeout,
            auth: cfg.auth.clone(),
            events,
            workers: Arc::new(Semaphore::new(cfg.workers)),
            workers_max: cfg.workers,
            aimd: std::sync::Mutex::new(None),
        })
    }

    #[must_use]
    pub fn workers(&self) -> Arc<Semaphore> {
        self.workers.clone()
    }

    /// The configured worker maximum.
    #[must_use]
    pub(crate) fn workers_max(&self) -> usize {
        self.workers_max
    }

    /// Attach the run's AIMD worker budget: the client reports every
    /// `Retry-After` failure and every success into it (§7).
    /// Called by the facade once per run; the budget lives as long as the run.
    pub fn attach_aimd(&self, limit: Arc<crate::transport::aimd::WorkerLimit>) {
        *self.aimd.lock().expect("aimd attach lock") = Some(limit);
    }

    /// The attached budget, if any.
    #[must_use]
    pub(crate) fn aimd(&self) -> Option<Arc<crate::transport::aimd::WorkerLimit>> {
        self.aimd.lock().expect("aimd attach lock").clone()
    }

    #[must_use]
    pub fn progress(&self) -> &Progress {
        &self.events
    }

    #[must_use]
    pub fn object_url(&self, dir: &str, name: &ArtifactName) -> String {
        format!("{dir}{}", name.encoded())
    }

    #[must_use]
    pub fn sibling_url(&self, dir: &str, name: &ArtifactName) -> String {
        format!("{dir}{}.sha256", name.encoded())
    }

    fn authorize(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            Some(v) => req.header(reqwest::header::AUTHORIZATION, v),
            None => req,
        }
    }

    /// The shared attempt loop: connect errors, timeouts, body breaks and 5xx retry.
    /// Each attempt builds a fresh boxed future borrowing `self` and `url` through the closure arguments, so nothing is carried between tries.
    async fn with_retries<T>(
        &self,
        subject: Option<(&str, Dir)>,
        url: &str,
        mut attempt: impl for<'a> FnMut(&'a Self, &'a str) -> Attempt<'a, T>,
    ) -> Result<T, Error> {
        let mut n = 0u32;
        loop {
            n += 1;
            match attempt(self, url).await {
                Ok(v) => {
                    // A full success also recovers the run's budget (§7):
                    // additive increase toward the configured maximum.
                    if let Some(aimd) = self.aimd() {
                        aimd.on_success();
                    }
                    return Ok(v);
                }
                Err(fail) if fail.retryable && n < self.retry.attempts => {
                    let reason = fail.error.to_string();
                    let pause = fail.retry_after.unwrap_or_else(|| self.retry.delay(n));
                    // A server-mandated pause means the server is under load:
                    // shrink the run's worker budget before the retry sleeps.
                    if fail.retry_after.is_some() {
                        if let Some(aimd) = self.aimd() {
                            aimd.on_retryable();
                        }
                    }
                    log::warn!(
                        "retry {n}/{} for {url}: {reason} (pause {pause:?})",
                        self.retry.attempts
                    );
                    let _ = self.events.sender().send(Event::Retrying {
                        name: subject
                            .map_or_else(|| Self::object_label(url), |(name, _)| name.to_owned()),
                        attempt: n + 1,
                        reason,
                    });
                    // tokio's timer needs a runtime, and wasm has none to enter:
                    // the backoff sleeps on the JS event loop instead.
                    #[cfg(not(target_arch = "wasm32"))]
                    tokio::time::sleep(pause).await;
                    #[cfg(target_arch = "wasm32")]
                    gloo_timers::future::sleep(pause).await;
                }
                Err(fail) => return Err(fail.error),
            }
        }
    }

    fn wrap_send_err(&self, url: &str, e: reqwest::Error) -> AttemptFailure {
        AttemptFailure {
            retryable: is_retryable(&e),
            retry_after: None,
            error: Error::transport(url, e),
        }
    }

    /// A non-success status: 5xx and 429 are retryable, and any retryable
    /// status honors the `Retry-After` pause the server asked for (a Nexus
    /// restart or a proxy overload answers `503 Retry-After: 30`).
    /// A 401/403 is the auth verdict.
    async fn status_failure(&self, url: &str, resp: reqwest::Response) -> AttemptFailure {
        let status = resp.status();
        let retryable = (500..600).contains(&status.as_u16()) || status.as_u16() == 429;
        let retry_after = retryable.then(|| Self::retry_after(&resp)).flatten();
        let error = match status.as_u16() {
            401 | 403 => Error::Auth {
                url: url.to_owned(),
                reason: format!(
                    "HTTP {status}; pass -u user:pass or export NXR_AUTH (base64 user:pass)"
                ),
                detail: Self::body_detail(resp).await,
            },
            s if retryable => Error::transport(url, format!("HTTP {s}")),
            s => Error::Http {
                status: s,
                url: url.to_owned(),
            },
        };
        AttemptFailure {
            retryable,
            retry_after,
            error,
        }
    }

    /// The `Retry-After` pause of a retryable status: the seconds form only, clamped to 1..=60.
    /// The HTTP-date form and unparseable values fall back to the regular backoff.
    fn retry_after(resp: &reqwest::Response) -> Option<std::time::Duration> {
        let raw = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)?
            .to_str()
            .ok()?;
        let secs: u64 = raw.trim().parse().ok()?;
        Some(std::time::Duration::from_secs(secs.clamp(1, 60)))
    }

    /// The server's error body as one short line for `Error::detail` (spec §5).
    /// An empty or unreadable body carries no detail.
    async fn body_detail(resp: reqwest::Response) -> Option<String> {
        // A hostile 401/403 body can be endless: read a small window, never the
        // whole body, then keep the first line.
        let mut stream = resp.bytes_stream();
        let cap = BODY_DETAIL_CHARS * 4;
        let mut buf = Vec::new();
        while buf.len() < cap {
            let chunk = match stream.next().await {
                Some(Ok(chunk)) => chunk,
                Some(Err(_)) | None => break,
            };
            let room = cap - buf.len();
            buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.lines().next()?.trim();
        if line.is_empty() {
            return None;
        }
        Some(line.chars().take(BODY_DETAIL_CHARS).collect())
    }

    /// The object a URL addresses, for callers that transfer by URL rather than by [`ArtifactName`]: markers, channel tokens, search pages.
    /// The label is the last path segment, which is the name these objects carry on the wire.
    /// Callers that know the artifact name pass it as a subject instead, so this only ever names small objects.
    fn object_label(url: &str) -> String {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        match path.trim_end_matches('/').rsplit('/').next() {
            Some(segment) if !segment.is_empty() => segment.to_owned(),
            _ => path.to_owned(),
        }
    }

    /// HEAD with full metadata.
    /// 404 is `HeadInfo { status: 404, .. }`.
    /// The size comes from the raw header: hyper reports a zero body size hint for HEAD responses regardless of Content-Length.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn head_info(&self, url: &str) -> Result<HeadInfo, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.head(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status().as_u16();
                let mut info = HeadInfo {
                    status,
                    size: None,
                    content_type: resp
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned),
                };
                if (200..300).contains(&status) {
                    info.size = resp
                        .headers()
                        .get(reqwest::header::CONTENT_LENGTH)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                }
                Ok(info)
            })
        })
        .await
    }

    /// GET a small object (sibling, channel): None on 404.
    ///
    /// The response is capped at `SMALL_CAP`: these objects are KiB-scale by protocol, and an unbounded slurp would turn a runaway or hostile server into a memory event.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET and [`Error::Misuse`] when the body exceeds the small-object cap.
    /// The range probe of `doctor --deep`: one ranged GET (`bytes=0-0`), no
    /// retry loop, the answer reduced to the verdict inputs.
    ///
    /// `206` means the server honors `Range`; `200` means the server (or a
    /// proxy on the way) strips it; anything else is named as-is.
    /// Read-only: a `bytes=0-0` GET transfers one byte and writes nothing.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the single attempt.
    pub async fn probe_range(&self, url: &str) -> Result<u16, Error> {
        let req = self
            .authorize(self.http.get(url))
            .header("Range", "bytes=0-0");
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Err(self.wrap_send_err(url, e).error),
        };
        Ok(resp.status().as_u16())
    }

    pub async fn get_small(&self, url: &str) -> Result<Option<Vec<u8>>, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => {
                        if resp.content_length().is_some_and(|n| n > SMALL_CAP) {
                            return Err(AttemptFailure::stop(Error::SmallObjectCap {
                                url: url.to_owned(),
                                cap: SMALL_CAP,
                            }));
                        }
                        let mut body = resp.bytes_stream();
                        let mut buf = Vec::new();
                        while let Some(chunk) = body.next().await {
                            let chunk = chunk.map_err(|e| this.wrap_send_err(url, e))?;
                            if buf.len() as u64 + chunk.len() as u64 > SMALL_CAP {
                                return Err(AttemptFailure::stop(Error::SmallObjectCap {
                                    url: url.to_owned(),
                                    cap: SMALL_CAP,
                                }));
                            }
                            buf.extend_from_slice(&chunk);
                        }
                        Ok(Some(buf))
                    }
                    _s if _s.as_u16() == 404 => Ok(None),
                    _ => Err(this.status_failure(url, resp).await),
                }
            })
        })
        .await
    }

    /// GET a byte range into memory.
    ///
    /// The mount read path: a `Range` request answered with `206` returns the span,
    /// a `200` means the server ignored `Range` and the stream is skipped to the span,
    /// a `404` returns `None`, and a `416` (the span starts at or past the end) returns an empty body.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn get_range(
        &self,
        url: &str,
        start: u64,
        end_incl: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this
                    .authorize(this.http.get(url))
                    .header(reqwest::header::RANGE, format!("bytes={start}-{end_incl}"))
                    // Byte offsets are only meaningful against the stored representation.
                    .header(reqwest::header::ACCEPT_ENCODING, "identity");
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status().as_u16();
                match status {
                    200 | 206 => {}
                    404 => return Ok(None),
                    416 => {
                        // The span starts at or past the object's end: a read past EOF is empty.
                        return Ok(Some(Vec::new()));
                    }
                    _ => return Err(this.status_failure(url, resp).await),
                }
                // 206: the body starts at `start`. 200: the server ignored
                // `Range`, so skip to `start` without buffering the object.
                let take = end_incl.saturating_sub(start).saturating_add(1);
                let skip = if status == 206 { 0 } else { start };
                Ok(Some(this.read_span(resp, skip, take, url).await?))
            })
        })
        .await
    }

    /// Read at most `take` bytes from `skip` on, with the stall window per chunk.
    /// A span read never buffers the whole object: a `Range`-stripping proxy
    /// answers `200`, and the skip walks the stream instead.
    async fn read_span(
        &self,
        resp: reqwest::Response,
        skip: u64,
        take: u64,
        url: &str,
    ) -> Result<Vec<u8>, AttemptFailure> {
        let mut stream = resp.bytes_stream();
        let mut skipped = 0u64;
        let mut out = Vec::new();
        while (out.len() as u64) < take {
            let next = tokio::time::timeout(self.stall, stream.next()).await;
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(e))) => return Err(self.wrap_send_err(url, e)),
                Ok(None) => break,
                Err(_) => {
                    return Err(AttemptFailure::again(Error::transport(
                        url,
                        format!("no bytes for {:?} while reading a range", self.stall),
                    )));
                }
            };
            let mut offset = 0usize;
            if skipped < skip {
                let need = skip - skipped;
                if (chunk.len() as u64) <= need {
                    skipped += chunk.len() as u64;
                    continue;
                }
                offset = usize::try_from(need).unwrap_or(usize::MAX);
                skipped = skip;
            }
            let want = usize::try_from(take - out.len() as u64).unwrap_or(0);
            let end = (offset + want).min(chunk.len());
            out.extend_from_slice(&chunk[offset..end]);
        }
        Ok(out)
    }

    /// PUT a small object whole.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn put_small(&self, url: &str, bytes: Vec<u8>) -> Result<(), Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            let bytes = bytes.clone();
            Box::pin(async move {
                let req = this.authorize(this.http.put(url)).body(bytes);
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                if status.is_success() {
                    return Ok(());
                }
                Err(this.status_failure(url, resp).await)
            })
        })
        .await
    }

    /// POST a small JSON body and require a success status.
    ///
    /// The service API uses POST for verbs (the EULA acceptance), unlike storage, which is PUT-only.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    pub async fn post_small(&self, url: &str, bytes: Vec<u8>) -> Result<(), Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            let bytes = bytes.clone();
            Box::pin(async move {
                let req = this
                    .authorize(this.http.post(url))
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(bytes);
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                if status.is_success() {
                    return Ok(());
                }
                Err(this.status_failure(url, resp).await)
            })
        })
        .await
    }

    /// DELETE a URL (§5.4): 2xx is deleted, 404 is already gone.
    ///
    /// Deletion is idempotent: the 404 of a rerun is a normal result, not an error.
    /// A 403/405 is the read-only repository: refused as [`Error::ReadOnly`] (exit 1), never retried.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReadOnly`] on a 403/405 and transport, auth or HTTP errors otherwise.
    pub async fn delete_url(&self, url: &str) -> Result<DeleteOutcome, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.delete(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status.as_u16() {
                    s if (200..300).contains(&s) => Ok(DeleteOutcome::Deleted),
                    404 => Ok(DeleteOutcome::Missing),
                    s @ (403 | 405) => {
                        let detail = Self::body_detail(resp).await;
                        Err(AttemptFailure::stop(Error::ReadOnly {
                            url: url.to_owned(),
                            status: s,
                            detail,
                        }))
                    }
                    _ => Err(this.status_failure(url, resp).await),
                }
            })
        })
        .await
    }

    /// Remote state of a name: HEAD of bytes + GET of sibling (§5.2).
    ///
    /// A sibling without bytes is ignored: the object is not complete.
    /// Classification of the HEAD: 404 is [`RemoteStatus::Absent`], 401/403 surface as [`Error::Auth`].
    /// A 2xx without `Content-Length` (and any other answered-but-unverifiable status) is [`RemoteStatus::Broken`], never [`RemoteStatus::Absent`]: a proxy could skip the digest comparison and the never-overwrite rule.
    /// Transient statuses (5xx, connection breaks) ride the shared retry loop like any other request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] on a 401/403 and transport or HTTP errors after the retry loop is exhausted.
    /// An unparseable sibling is the [`RemoteStatus::Broken`] result, not an error.
    pub async fn probe(&self, dir: &str, name: &ArtifactName) -> Result<RemoteStatus, Error> {
        let bytes_url = self.object_url(dir, name);
        let info = self
            .with_retries(
                Some((name.as_str(), Dir::Down)),
                &bytes_url,
                |this: &Self, url: &str| {
                    Box::pin(async move {
                        // The raw HEAD, inline: probe owns the only retry loop, so connection errors and 5xx share one attempt budget instead of nesting two.
                        let resp = this
                            .authorize(this.http.head(url))
                            .send()
                            .await
                            .map_err(|e| this.wrap_send_err(url, e))?;
                        let status = resp.status().as_u16();
                        let info = HeadInfo {
                            status,
                            size: if (200..300).contains(&status) {
                                resp.headers()
                                    .get(reqwest::header::CONTENT_LENGTH)
                                    .and_then(|v| v.to_str().ok())
                                    .and_then(|v| v.parse::<u64>().ok())
                            } else {
                                None
                            },
                            content_type: resp
                                .headers()
                                .get(reqwest::header::CONTENT_TYPE)
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_owned),
                        };
                        match status {
                            s if (200..300).contains(&s) => Ok(Some(info)),
                            404 => Ok(None),
                            // Everything else routes through the shared verdict: auth, rate limits, 5xx.
                            _ => Err(this.status_failure(url, resp).await),
                        }
                    })
                },
            )
            .await?;
        let Some(info) = info else {
            return Ok(RemoteStatus::Absent);
        };
        let size = match info.size_filter_ok() {
            Some(size) => Some(size),
            None => {
                return Ok(RemoteStatus::Broken(
                    "HEAD answered 2xx without Content-Length; the object is \
                     present but unverifiable"
                        .to_owned(),
                ))
            }
        };
        let sib_url = self.sibling_url(dir, name);
        match self.get_small(&sib_url).await? {
            None => Ok(RemoteStatus::Markerless { size }),
            Some(raw) => match sibling::parse_line(&String::from_utf8_lossy(&raw)) {
                Ok(s) => Ok(RemoteStatus::Complete {
                    digest: s.digest,
                    size,
                }),
                Err(e) => Ok(RemoteStatus::Broken(e)),
            },
        }
    }

    /// GET a URL body as a chunk stream: no hashing, no completion decision.
    ///
    /// The single-attempt body stream for stdout mode: once the body started arriving, a retry would duplicate bytes, so breaks surface as errors.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors for the request.
    /// Body breaks after the headers surface when the caller reads the stream.
    pub async fn get_stream(&self, url: &str) -> Result<reqwest::Response, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => Ok(resp),
                    _ => Err(this.status_failure(url, resp).await),
                }
            })
        })
        .await
    }

    /// GET into a part file with Range resume (§5.2).
    ///
    /// `cont == true` and an existing `part` continue from its size through `Range: bytes=N-`.
    /// A server that answers `200` (range ignored) restarts from zero.
    /// A `416` (range already satisfied) finalizes the part: the crash-between-download-end-and-rename edge.
    /// Verifying the finalized bytes is the caller's duty: `down` digest-checks against the sibling, the `get` primitive trusts the part.
    /// The digest covers the whole file, prefix included.
    /// Returns the final file size and digest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the part file cannot be opened, read or written, and transport, auth or HTTP errors after the retry loop is exhausted.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_resumable(
        &self,
        subject: (&str, Dir),
        url: &str,
        part: &Path,
        resume: bool,
        total_hint: Option<u64>,
    ) -> Result<(u64, Digest), Error> {
        let stall = self.stall;
        let progress = self.events.clone();
        let subject_name = subject.0.to_owned();
        self.with_retries(Some(subject), url, |this: &Self, url: &str| {
            let part: PathBuf = part.to_owned();
            let progress = progress.clone();
            let subject_name = subject_name.clone();
            Box::pin(async move {
                // The prefix: existing part content when resuming.
                // symlink_metadata, not metadata: a symlink at the predictable
                // part path must never be read as resume state (the write open
                // already refuses to follow one, the read side must too).
                let mut prefix: u64 = 0;
                let mut hasher = Sha256::new();
                if resume {
                    match tokio::fs::symlink_metadata(&part).await {
                        Ok(meta) if meta.is_file() => {
                            let mut f = tokio::fs::File::open(&part)
                                .await
                                .map_err(|e| AttemptFailure::stop(Error::io(&part, e)))?;
                            let mut buf = [0u8; 64 * 1024];
                            loop {
                                match f.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        hasher.update(&buf[..n]);
                                        prefix += n as u64;
                                    }
                                    Err(e) => {
                                        return Err(AttemptFailure::stop(Error::io(&part, e)));
                                    }
                                }
                            }
                        }
                        Ok(_) => {
                            return Err(AttemptFailure::stop(Error::io(
                                &part,
                                std::io::Error::other(
                                    "the part path is not a regular file",
                                ),
                            )));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(AttemptFailure::stop(Error::io(&part, e)));
                        }
                    }
                }
                // One attempt may restart from zero once: a stale part or a
                // shifted range is deterministic, not a transient failure, and
                // must not spend the caller's retry budget.
                let mut restarted = false;
                let mut range_end_exclusive: Option<u64> = None;
                let resp = loop {
                    let mut req = this.authorize(this.http.get(url));
                    if prefix > 0 {
                        // Byte offsets are only meaningful against the stored representation.
                        // The resume request must not negotiate a content-encoding.
                        req = req
                            .header(reqwest::header::RANGE, format!("bytes={prefix}-"))
                            .header(reqwest::header::ACCEPT_ENCODING, "identity");
                    }
                    let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                    let status = resp.status().as_u16();
                    if status == 416 && prefix > 0 {
                        // The server refuses the range: the part is at or past the
                        // stored object's end. Finalize only when the file on disk
                        // still holds exactly the bytes that were hashed; otherwise
                        // the part is stale or was changed under us, and resuming
                        // it would rename bytes that the object never carried.
                        // The 416 header is the current truth about the object
                        // size: `Content-Range: bytes */total`. Only when it is
                        // absent or unparseable does the caller's hint or a HEAD
                        // stand in.
                        let declared = resp
                            .headers()
                            .get(reqwest::header::CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(content_range_total);
                        let total = match declared.or(total_hint) {
                            Some(t) => Some(t),
                            None => this.head_info(url).await.ok().and_then(|info| info.size),
                        };
                        let on_disk = tokio::fs::symlink_metadata(&part)
                            .await
                            .ok()
                            .filter(std::fs::Metadata::is_file)
                            .map(|m| m.len());
                        if total == Some(prefix) && on_disk == Some(prefix) {
                            // The crash-between-download-end-and-rename edge: the part
                            // holds the whole object, the caller digest-checks and renames.
                            let digest = Digest::from_hex_string(crate::model::digest::hex(
                                &hasher.finalize(),
                            ));
                            return Ok((prefix, digest));
                        }
                        if restarted {
                            return Err(AttemptFailure::stop(Error::transport(
                                url,
                                "the server keeps refusing the range after a restart from zero",
                            )));
                        }
                        tokio::fs::remove_file(&part).await.ok();
                        let total_text = total.map_or_else(
                            || "an unknown size".to_owned(),
                            |t| format!("{t} bytes"),
                        );
                        let disk_text = on_disk.map_or_else(
                            || "no regular file".to_owned(),
                            |n| format!("{n} bytes on disk"),
                        );
                        log::warn!(
                            "the existing part is stale ({prefix} bytes hashed, {disk_text}, object {total_text}): restarting from zero"
                        );
                        prefix = 0;
                        hasher = Sha256::new();
                        restarted = true;
                        continue;
                    }
                    if !(200..300).contains(&status) {
                        return Err(this.status_failure(url, resp).await);
                    }
                    // 206 = the range was honored, append from `prefix`.
                    // 200 = full body, restart from zero.
                    let append = status == 206 && prefix > 0;
                    if append {
                        // A 206 must start exactly where the part ends and cover
                        // the whole remaining suffix. A proxy that shifts or
                        // shortens the span would otherwise assemble bytes that no
                        // digest ever described.
                        let parsed = resp
                            .headers()
                            .get(reqwest::header::CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(parse_content_range);
                        let range_ok = parsed.and_then(|(start, end, total)| {
                            let end_exclusive = end.checked_add(1)?;
                            (start == prefix
                                && total.is_none_or(|t| total_hint.is_none_or(|hint| hint == t))
                                && total.or(total_hint).is_none_or(|t| end_exclusive == t))
                            .then_some(end_exclusive)
                        });
                        let Some(end_exclusive) = range_ok else {
                            if restarted {
                                return Err(AttemptFailure::stop(Error::transport(
                                    url,
                                    "the ranged answer still does not cover the part offset after a restart",
                                )));
                            }
                            tokio::fs::remove_file(&part).await.ok();
                            prefix = 0;
                            hasher = Sha256::new();
                            restarted = true;
                            continue;
                        };
                        range_end_exclusive = Some(end_exclusive);
                    }
                    break resp;
                };
                let status = resp.status().as_u16();
                let append = status == 206 && prefix > 0;
                if !append {
                    prefix = 0;
                    hasher = Sha256::new();
                }
                // `content_length()` is an exact body size hint: it is absent
                // for a chunked or transparently decompressed body, where the
                // encoded length on the wire says nothing about the bytes the
                // stream yields. Only an exact hint can prove a cut transfer.
                let exact = match resp.content_length() {
                    Some(n) => Some(n.checked_add(prefix).ok_or_else(|| {
                        AttemptFailure::stop(Error::transport(
                            url,
                            "the Content-Length plus the part offset overflows",
                        ))
                    })?),
                    None => None,
                };
                let total = exact.or(total_hint);
                progress.started(&subject_name, Dir::Down, total).await;
                let mut body = resp.bytes_stream();
                let mut file = write_options(append)
                    .open(&part)
                    .await
                    .map_err(|e| AttemptFailure::stop(Error::io(&part, e)))?;
                let mut done: u64 = prefix;
                loop {
                    let chunk = tokio::time::timeout(stall, body.next()).await;
                    let chunk = match chunk {
                        Ok(Some(c)) => c,
                        Ok(None) => break,
                        Err(_) => {
                            let e = format!("stalled: no bytes for {}s", stall.as_secs_f64());
                            return Err(AttemptFailure::again(Error::transport(url, e)));
                        }
                    };
                    let chunk = chunk.map_err(|e| this.wrap_send_err(url, e))?;
                    hasher.update(&chunk);
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| AttemptFailure::stop(Error::io(&part, e)))?;
                    done += chunk.len() as u64;
                    progress.bytes(&subject_name, Dir::Down, done, total).await;
                }
                file.flush()
                    .await
                    .map_err(|e| AttemptFailure::stop(Error::io(&part, e)))?;
                // The digest covers the stream, the caller renames the file:
                // they must describe the same bytes. A concurrent writer that
                // truncated or extended the part fails here instead of landing
                // a file no digest vouched for.
                let on_disk = file
                    .metadata()
                    .await
                    .map_err(|e| AttemptFailure::stop(Error::io(&part, e)))?
                    .len();
                if on_disk != done {
                    return Err(AttemptFailure::again(Error::transport(
                        url,
                        format!(
                            "the part on disk is {on_disk} bytes after the stream wrote {done}: retrying"
                        ),
                    )));
                }
                // The `Content-Range` end is authoritative even when the body
                // is chunked and carries no exact `Content-Length`: a cleanly
                // terminated short span is still a truncated download.
                if let Some(expected_end) = range_end_exclusive {
                    if done != expected_end {
                        return Err(AttemptFailure::again(Error::transport(
                            url,
                            format!(
                                "the ranged body ended at {done} of {expected_end} bytes: retrying"
                            ),
                        )));
                    }
                }
                // A body that ends short of an exact advertised size is a cut
                // transfer, never a complete object: retry instead of renaming.
                if let Some(expected) = exact {
                    if done != expected {
                        return Err(AttemptFailure::again(Error::transport(
                            url,
                            format!("the body ended at {done} of {expected} bytes: retrying"),
                        )));
                    }
                }
                let digest = Digest::from_hex_string(crate::model::digest::hex(&hasher.finalize()));
                Ok((done, digest))
            })
        })
        .await
    }

    /// GET one closed byte span into a fresh file (segmented download).
    ///
    /// Sends `Range: bytes=start-end` (inclusive on both ends) and requires the
    /// `206` + `Content-Range` answer: the whole-span body streams into `file`,
    /// which must not exist yet, so a retried attempt never mixes two fetches.
    /// A `416` means the span is empty on the server side (an object that shrank
    /// between HEAD and GET): a retryable transport failure, the caller's retries
    /// re-HEAD and re-split through a fresh call.
    ///
    /// Returns the exact byte count written.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted, and [`Error::Io`] when `file` cannot be written.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn fetch_range(
        &self,
        subject: (&str, Dir),
        url: &str,
        start: u64,
        end_incl: u64,
        file: &Path,
    ) -> Result<u64, Error> {
        let stall = self.stall;
        let progress = self.events.clone();
        let subject_name = subject.0.to_owned();
        let file: PathBuf = file.to_owned();
        let expected = end_incl - start + 1;
        self.with_retries(Some(subject), url, |this: &Self, url| {
            let file = file.clone();
            let progress = progress.clone();
            let subject_name = subject_name.clone();
            Box::pin(async move {
                // A leftover from a failed attempt must never mix with a fresh fetch.
                let _ = tokio::fs::remove_file(&file).await;
                let req = this.authorize(this.http.get(url));
                let req = req
                    .header(
                        reqwest::header::RANGE,
                        format!("bytes={start}-{end_incl}"),
                    )
                    // Byte offsets are only meaningful against the stored representation.
                    .header(reqwest::header::ACCEPT_ENCODING, "identity");
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status().as_u16();
                match status {
                    206 => {}
                    // The whole-body answer: the server ignored the Range header.
                    200 => return Err(AttemptFailure::stop(Error::RangeIgnored { url: url.to_owned() })),
                    // The span starts at or past the object's end: the object changed size under us.
                    416 => {
                        return Err(AttemptFailure::again(Error::transport(
                            url,
                            format!("HTTP 416 for bytes={start}-{end_incl}: the object changed size under the segmented download"),
                        )));
                    }
                    _ => return Err(this.status_failure(url, resp).await),
                }
                // A 206 without a length cannot be positioned: refuse instead of assembling short.
                let Some(length) = resp.content_length() else {
                    return Err(AttemptFailure::stop(Error::transport(
                        url,
                        "the 206 answer carries no Content-Length: the span cannot be staged",
                    )));
                };
                if length != expected {
                    return Err(AttemptFailure::stop(Error::transport(
                        url,
                        format!(
                            "the 206 answer carries {length} bytes, the span asks for {expected}"
                        ),
                    )));
                }
                progress.started(&subject_name, Dir::Down, Some(length)).await;
                let mut body = resp.bytes_stream();
                let mut out = write_options(false)
                    .open(&file)
                    .await
                    .map_err(|e| AttemptFailure::stop(Error::io(&file, e)))?;
                let mut done: u64 = 0;
                loop {
                    let chunk = tokio::time::timeout(stall, body.next()).await;
                    let chunk = match chunk {
                        Ok(Some(c)) => c,
                        Ok(None) => break,
                        Err(_) => {
                            let e = format!("stalled: no bytes for {}s", stall.as_secs_f64());
                            return Err(AttemptFailure::again(Error::transport(url, e)));
                        }
                    };
                    let chunk = chunk.map_err(|e| this.wrap_send_err(url, e))?;
                    out.write_all(&chunk)
                        .await
                        .map_err(|e| AttemptFailure::stop(Error::io(&file, e)))?;
                    done += chunk.len() as u64;
                    progress.bytes(&subject_name, Dir::Down, done, Some(length)).await;
                }
                out.flush()
                    .await
                    .map_err(|e| AttemptFailure::stop(Error::io(&file, e)))?;
                if done != expected {
                    return Err(AttemptFailure::again(Error::transport(
                        url,
                        format!("the body broke after {done} of {expected} bytes"),
                    )));
                }
                Ok(done)
            })
        })
        .await
    }

    /// PUT the bytes of a file: streamed body with Content-Length.
    /// Stall detection is attempt-level: no chunk pulled for `stall` fails the attempt as retryable transport, so a frozen socket surfaces even after the producer has finished reading the file.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors after the retry loop is exhausted.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_file(
        &self,
        subject: (&str, Dir),
        url: &str,
        src: &Path,
        size: u64,
    ) -> Result<Digest, Error> {
        let stall = self.stall;
        let progress = self.events.clone();
        let subject_name = subject.0.to_owned();
        self.with_retries(Some(subject), url, |this: &Self, url: &str| {
            let src = src.to_owned();
            let progress = progress.clone();
            let subject_name = subject_name.clone();
            Box::pin(async move {
                let (tx, mut rx) = mpsc::channel::<std::io::Result<Vec<u8>>>(UPLOAD_CHANNEL);
                // When the body last moved: a chunk was pulled for the wire.
                let last = Arc::new(Mutex::new(std::time::Instant::now()));
                let producer_src = src.clone();
                let producer_progress = progress.clone();
                let producer_subject = subject_name.clone();
                // The size in the plan may be older than the file: a build that
                // rewrote it between classification and send would make the body
                // disagree with Content-Length. Re-stat per attempt, and fail
                // before a single byte reaches the wire.
                let actual = match tokio::fs::metadata(&src).await {
                    Ok(meta) => meta.len(),
                    Err(e) => return Err(AttemptFailure::stop(Error::io(&src, e))),
                };
                if actual != size {
                    return Err(AttemptFailure::stop(Error::io(
                        &src,
                        std::io::Error::other(format!(
                            "the source changed since it was planned: {size} bytes planned, {actual} on disk"
                        )),
                    )));
                }
                let producer = tokio::spawn(async move {
                    let mut file = match tokio::fs::File::open(&producer_src).await {
                        Ok(f) => f,
                        Err(e) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    };
                    let mut produced: u64 = 0;
                    loop {
                        // Read exactly the declared size: a file that grew
                        // after the plan contributes its planned prefix, never
                        // a body that outruns Content-Length.
                        if produced == size {
                            break;
                        }
                        let want = (size - produced).min(UPLOAD_CHUNK as u64) as usize;
                        let mut buf = vec![0u8; want];
                        match file.read(&mut buf).await {
                            Ok(0) => {
                                // A file that shrank: the body cannot satisfy
                                // the declared length. The next attempt re-stats
                                // and reports the change as an I/O error.
                                let _ = tx
                                    .send(Err(std::io::Error::other(
                                        "the source changed during the upload: the file ended early",
                                    )))
                                    .await;
                                return;
                            }
                            Ok(n) => {
                                buf.truncate(n);
                                produced += n as u64;
                                // A full channel is backpressure: the consumer is busy on the wire.
                                // The attempt-level watchdog owns the stall timeout, so this send cannot deadlock.
                                if tx.send(Ok(buf)).await.is_err() {
                                    // Receiver gone: the attempt was cancelled.
                                    break;
                                }
                                producer_progress
                                    .bytes(&producer_subject, Dir::Up, produced, Some(size))
                                    .await;
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e)).await;
                                return;
                            }
                        }
                    }
                });
                progress.started(&subject_name, Dir::Up, Some(size)).await;
                let watcher = last.clone();
                // The digest of what actually hit the wire: the consumer hashes the
                // chunks it feeds the body, so put never re-reads the file for a marker.
                let upload_hash = Arc::new(std::sync::Mutex::new(Sha256::new()));
                let body_hasher = upload_hash.clone();
                let body = reqwest::Body::wrap_stream(futures_util::stream::poll_fn(move |cx| {
                    use futures_util::task::Poll;
                    match rx.poll_recv(cx) {
                        Poll::Ready(Some(Ok(chunk))) => {
                            *watcher.lock().expect("progress lock") = std::time::Instant::now();
                            body_hasher
                                .lock()
                                .expect("upload digest lock")
                                .update(&chunk);
                            Poll::Ready(Some(
                                Ok::<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>(chunk),
                            ))
                        }
                        Poll::Ready(Some(Err(e))) => {
                            Poll::Ready(Some(Err(
                                Box::<dyn std::error::Error + Send + Sync>::from(e),
                            )))
                        }
                        Poll::Ready(None) => Poll::Ready(None),
                        Poll::Pending => Poll::Pending,
                    }
                }));
                let req = this
                    .authorize(this.http.put(url))
                    .header(reqwest::header::CONTENT_LENGTH, size)
                    .body(body);
                // The attempt owns stall detection: no chunk pulled for `stall` fails the attempt as retryable transport, whether the producer is alive, already done, or the socket stopped draining.
                let mut send = std::pin::pin!(req.send());
                let resp = loop {
                    let since_progress = last.lock().expect("progress lock").elapsed();
                    tokio::select! {
                        biased;
                        result = &mut send => {
                            break result.map_err(|e| this.wrap_send_err(url, e));
                        }
                        () = tokio::time::sleep(stall.saturating_sub(since_progress)) => {
                            if last.lock().expect("progress lock").elapsed() >= stall {
                                producer.abort();
                                break Err(AttemptFailure::again(Error::transport(
                                    url,
                                    format!(
                                        "stalled: no upload progress for {}s",
                                        stall.as_secs_f64()
                                    ),
                                )));
                            }
                            // Progress raced the timer: re-arm and keep waiting.
                        }
                    }
                };
                producer.abort();
                let resp = resp?;
                let status = resp.status();
                match status {
                    s if s.is_success() => {
                        let hasher = upload_hash.lock().expect("upload digest lock").clone();
                        Ok(Digest::from_hex_string(crate::model::digest::hex(
                            &hasher.finalize(),
                        )))
                    }
                    _ => Err(this.status_failure(url, resp).await),
                }
            })
        })
        .await
    }
}

impl HeadInfo {
    fn size_filter_ok(self) -> Option<u64> {
        if (200..300).contains(&self.status) {
            self.size
        } else {
            None
        }
    }
}
