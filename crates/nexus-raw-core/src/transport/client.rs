//! `NexusClient`: GET/HEAD/PUT over reqwest.
//! Auth, TLS, retries, stall detection, Range resume (spec §5.1).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};

use crate::config::Config;
use crate::error::Error;
use crate::events::{Dir, Event, Progress};
use crate::model::digest::Digest;
use crate::model::name::ArtifactName;
use crate::model::sibling;
use crate::model::state::RemoteStatus;
use crate::transport::retry::{is_retryable, AttemptFailure, RetryPolicy};

/// Outgoing body chunk size: small enough that a full outbound buffer drains well inside the stall timeout, so channel backpressure means a real stall.
const UPLOAD_CHUNK: usize = 16 * 1024;
/// Depth of the outgoing body channel.
const UPLOAD_CHANNEL: usize = 4;

type Attempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttemptFailure>> + Send + 'a>>;

/// Upper bound for "small" GETs: siblings, manifests, channel tokens, search pages.
const SMALL_CAP: u64 = 16 * 1024 * 1024;

/// Open options for files nxr writes under a server-name-derived path (`.part` files, local `.sha256` markers): create/truncate as asked, but never follow a symlink — a predictable name must not become a write into a different file.
///
/// The append arm deliberately lacks `create`: it may only open a part whose prefix was just read.
/// A part that vanished in between must fail loudly — a silently recreated file would yield a truncated "complete" artifact whose digest still matches (the hash covers the prefix that was read, not the bytes on disk).
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
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
}

/// The blocking counterpart of `write_options` for `spawn_blocking` code.
#[cfg(unix)]
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
#[cfg(not(unix))]
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
}

impl NexusClient {
    /// Build the client from `cfg`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when [`Config::validate`] refuses the config or the HTTP client cannot be built.
    pub fn new(cfg: &Config, events: Progress) -> Result<Self, Error> {
        cfg.validate()?;
        let mut builder = reqwest::Client::builder()
            .connect_timeout(cfg.connect_timeout)
            .redirect(reqwest::redirect::Policy::none());
        if cfg.tls_insecure {
            builder = builder.danger_accept_invalid_certs(true);
        }
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
        })
    }

    #[must_use]
    pub fn workers(&self) -> Arc<Semaphore> {
        self.workers.clone()
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
                Ok(v) => return Ok(v),
                Err(fail) if fail.retryable && n < self.retry.attempts => {
                    let reason = fail.error.to_string();
                    log::warn!("retry {n}/{} for {url}: {reason}", self.retry.attempts);
                    let _ = self.events.sender().send(Event::Retrying {
                        name: subject
                            .map_or_else(|| Self::object_label(url), |(name, _)| name.to_owned()),
                        attempt: n + 1,
                        reason,
                    });
                    tokio::time::sleep(self.retry.delay(n)).await;
                }
                Err(fail) => return Err(fail.error),
            }
        }
    }

    fn wrap_send_err(&self, url: &str, e: reqwest::Error) -> AttemptFailure {
        AttemptFailure {
            retryable: is_retryable(&e),
            error: Error::transport(url, e),
        }
    }

    fn status_failure(&self, url: &str, status: reqwest::StatusCode) -> AttemptFailure {
        let retryable = (500..600).contains(&status.as_u16());
        let error = match status.as_u16() {
            401 | 403 => Error::Auth {
                url: url.to_owned(),
                reason: format!(
                    "HTTP {status}; pass -u user:pass or export NXR_AUTH (base64 user:pass)"
                ),
            },
            s if retryable => Error::transport(url, format!("HTTP {s}")),
            s => Error::Http {
                status: s,
                url: url.to_owned(),
            },
        };
        AttemptFailure { retryable, error }
    }

    /// The object a URL addresses, for callers that transfer by URL rather than by [`ArtifactName`]: markers, channel tokens, manifests, search pages.
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

    /// GET a small object (sibling, manifest, channel): None on 404.
    ///
    /// The response is capped at `SMALL_CAP`: these objects are KiB-scale by protocol, and an unbounded slurp would turn a runaway or hostile server into a memory event.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET and [`Error::Misuse`] when the body exceeds the small-object cap.
    pub async fn get_small(&self, url: &str) -> Result<Option<Vec<u8>>, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => {
                        if resp.content_length().is_some_and(|n| n > SMALL_CAP) {
                            return Err(AttemptFailure {
                                retryable: false,
                                error: Error::misuse(format!(
                                    "{url} exceeds the small-object cap ({SMALL_CAP} bytes)"
                                )),
                            });
                        }
                        let mut body = resp.bytes_stream();
                        let mut buf = Vec::new();
                        while let Some(chunk) = body.next().await {
                            let chunk = chunk.map_err(|e| this.wrap_send_err(url, e))?;
                            if buf.len() as u64 + chunk.len() as u64 > SMALL_CAP {
                                return Err(AttemptFailure {
                                    retryable: false,
                                    error: Error::misuse(format!(
                                        "{url} exceeds the small-object cap ({SMALL_CAP} bytes)"
                                    )),
                                });
                            }
                            buf.extend_from_slice(&chunk);
                        }
                        Ok(Some(buf))
                    }
                    s if s.as_u16() == 404 => Ok(None),
                    s => Err(this.status_failure(url, s)),
                }
            })
        })
        .await
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
                match status {
                    s if s.is_success() => Ok(()),
                    s => Err(this.status_failure(url, s)),
                }
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
                    s @ (403 | 405) => Err(AttemptFailure {
                        retryable: false,
                        error: Error::ReadOnly {
                            url: url.to_owned(),
                            status: s,
                        },
                    }),
                    s => Err(this.status_failure(url, reqwest::StatusCode::from_u16(s).unwrap())),
                }
            })
        })
        .await
    }

    /// Remote state of a name: HEAD of bytes + GET of sibling (§5.2).
    ///
    /// A sibling without bytes is ignored: the object is not complete.
    /// Classification of the HEAD: 404 is [`RemoteStatus::Absent`], 401/403 surface as [`Error::Auth`].
    /// A 2xx without `Content-Length` (and any other answered-but-unverifiable status) is [`RemoteStatus::Broken`] — never [`RemoteStatus::Absent`], or a proxy could skip the digest comparison and the never-overwrite rule.
    /// Transient statuses (5xx, connection breaks) ride the shared retry loop like any other request.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Auth`] on a 401/403 and transport or HTTP errors after the retry loop is exhausted; an unparseable sibling is the [`RemoteStatus::Broken`] result, not an error.
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
                            s @ (401 | 403) => {
                                Err(this
                                    .status_failure(url, reqwest::StatusCode::from_u16(s).unwrap()))
                            }
                            s if (500..600).contains(&s) => Err(AttemptFailure {
                                retryable: true,
                                error: Error::transport(url, format!("HTTP {s}")),
                            }),
                            s => Err(AttemptFailure {
                                retryable: false,
                                error: Error::Http {
                                    status: s,
                                    url: url.to_owned(),
                                },
                            }),
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
    /// Returns transport, auth or HTTP errors for the request; body breaks after the headers surface when the caller reads the stream.
    pub async fn get_stream(&self, url: &str) -> Result<reqwest::Response, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => Ok(resp),
                    s => Err(this.status_failure(url, s)),
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
                let mut prefix: u64 = 0;
                let mut hasher = Sha256::new();
                if resume {
                    match tokio::fs::File::open(&part).await {
                        Ok(mut f) => {
                            let mut buf = [0u8; 64 * 1024];
                            loop {
                                match f.read(&mut buf).await {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        hasher.update(&buf[..n]);
                                        prefix += n as u64;
                                    }
                                    Err(e) => {
                                        return Err(AttemptFailure {
                                            retryable: false,
                                            error: Error::io(&part, e),
                                        })
                                    }
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(AttemptFailure {
                                retryable: false,
                                error: Error::io(&part, e),
                            })
                        }
                    }
                }
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
                    // The server refuses the range because it is already satisfied: the part holds the whole object.
                    // This is the crash-between-download-end-and-rename edge: finalize the part and let the caller digest-check and rename.
                    let digest =
                        Digest::from_hex_string(crate::model::digest::hex(&hasher.finalize()));
                    return Ok((prefix, digest));
                }
                if !(200..300).contains(&status) {
                    return Err(this.status_failure(url, resp.status()));
                }
                // 206 = the range was honored, append from `prefix`.
                // 200 = full body, restart from zero.
                let append = status == 206 && prefix > 0;
                if !append {
                    prefix = 0;
                    hasher = Sha256::new();
                }
                let total = resp.content_length().map(|n| n + prefix).or(total_hint);
                progress.started(&subject_name, Dir::Down, total).await;
                let mut body = resp.bytes_stream();
                let mut file =
                    write_options(append)
                        .open(&part)
                        .await
                        .map_err(|e| AttemptFailure {
                            retryable: false,
                            error: Error::io(&part, e),
                        })?;
                let mut done: u64 = prefix;
                loop {
                    let chunk = tokio::time::timeout(stall, body.next()).await;
                    let chunk = match chunk {
                        Ok(Some(c)) => c,
                        Ok(None) => break,
                        Err(_) => {
                            let e = format!("stalled: no bytes for {}s", stall.as_secs_f64());
                            return Err(AttemptFailure {
                                retryable: true,
                                error: Error::transport(url, e),
                            });
                        }
                    };
                    let chunk = chunk.map_err(|e| this.wrap_send_err(url, e))?;
                    hasher.update(&chunk);
                    file.write_all(&chunk).await.map_err(|e| AttemptFailure {
                        retryable: false,
                        error: Error::io(&part, e),
                    })?;
                    done += chunk.len() as u64;
                    progress.bytes(&subject_name, Dir::Down, done, total).await;
                }
                file.flush().await.map_err(|e| AttemptFailure {
                    retryable: false,
                    error: Error::io(&part, e),
                })?;
                let digest = Digest::from_hex_string(crate::model::digest::hex(&hasher.finalize()));
                Ok((done, digest))
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
    pub async fn upload_file(
        &self,
        subject: (&str, Dir),
        url: &str,
        src: &Path,
        size: u64,
    ) -> Result<(), Error> {
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
                        let mut buf = vec![0u8; UPLOAD_CHUNK];
                        match file.read(&mut buf).await {
                            Ok(0) => break,
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
                let body = reqwest::Body::wrap_stream(futures_util::stream::poll_fn(move |cx| {
                    use futures_util::task::Poll;
                    match rx.poll_recv(cx) {
                        Poll::Ready(Some(Ok(chunk))) => {
                            *watcher.lock().expect("progress lock") = std::time::Instant::now();
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
                                break Err(AttemptFailure {
                                    retryable: true,
                                    error: Error::transport(url, format!(
                                        "stalled: no upload progress for {}s",
                                        stall.as_secs_f64()
                                    )),
                                });
                            }
                            // Progress raced the timer: re-arm and keep waiting.
                        }
                    }
                };
                producer.abort();
                let resp = resp?;
                let status = resp.status();
                match status {
                    s if s.is_success() => Ok(()),
                    s => Err(this.status_failure(url, s)),
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
