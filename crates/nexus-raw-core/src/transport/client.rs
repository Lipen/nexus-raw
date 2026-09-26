//! `NexusClient`: GET/HEAD/PUT over reqwest.
//! Auth, TLS, retries, stall detection, Range resume (spec §5.1).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
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

/// Outgoing body chunk size: small enough that a full outbound buffer drains
/// well inside the stall timeout, so channel backpressure means a real stall.
const UPLOAD_CHUNK: usize = 16 * 1024;
/// Depth of the outgoing body channel.
const UPLOAD_CHANNEL: usize = 4;

type Attempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttemptFailure>> + Send + 'a>>;

/// What a HEAD saw: the status and the advertised metadata.
/// A 404 is a normal result, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadInfo {
    pub status: u16,
    pub size: Option<u64>,
    pub content_type: Option<String>,
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

    pub fn workers(&self) -> Arc<Semaphore> {
        self.workers.clone()
    }

    pub fn progress(&self) -> &Progress {
        &self.events
    }

    pub fn object_url(&self, dir: &str, name: &ArtifactName) -> String {
        format!("{dir}{}", name.encoded())
    }

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
    /// Each attempt builds a fresh boxed future borrowing `self` and `url`
    /// through the closure arguments, so nothing is carried between tries.
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
                        name: subject.map(|(name, _)| name.to_owned()).unwrap_or_default(),
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

    /// HEAD an object: Some(content-length), or None on 404.
    ///
    /// The size comes from the raw header: hyper reports a zero body size hint
    /// for HEAD responses regardless of Content-Length.
    pub async fn head(&self, url: &str) -> Result<Option<u64>, Error> {
        Ok(self.head_info(url).await?.size_filter_ok())
    }

    /// HEAD with full metadata; 404 is `HeadInfo { status: 404, .. }`.
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
    pub async fn get_small(&self, url: &str) -> Result<Option<Vec<u8>>, Error> {
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => Ok(Some(
                        resp.bytes()
                            .await
                            .map_err(|e| this.wrap_send_err(url, e))?
                            .to_vec(),
                    )),
                    s if s.as_u16() == 404 => Ok(None),
                    s => Err(this.status_failure(url, s)),
                }
            })
        })
        .await
    }

    /// PUT a small object whole.
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

    /// Remote state of a name: HEAD of bytes + GET of sibling (§5.2).
    ///
    /// A sibling without bytes is ignored — the object is not complete.
    pub async fn probe(&self, dir: &str, name: &ArtifactName) -> Result<RemoteStatus, Error> {
        let bytes_url = self.object_url(dir, name);
        let Some(size) = self.head(&bytes_url).await? else {
            return Ok(RemoteStatus::Absent);
        };
        let sib_url = self.sibling_url(dir, name);
        match self.get_small(&sib_url).await? {
            None => Ok(RemoteStatus::Markerless { size: Some(size) }),
            Some(raw) => match sibling::parse_line(&String::from_utf8_lossy(&raw)) {
                Ok(s) => Ok(RemoteStatus::Complete {
                    digest: s.digest,
                    size: Some(size),
                }),
                Err(e) => Ok(RemoteStatus::Broken(e)),
            },
        }
    }

    /// GET a body as a stream of chunks, hashing nothing, deciding nothing.
    ///
    /// The single-attempt body stream for stdout mode: once the body started
    /// arriving, a retry would duplicate bytes, so breaks surface as errors.
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
    /// `cont == true` and an existing `part` continue from its size through
    /// `Range: bytes=N-`; a server that answers `200` (range ignored) restarts
    /// from zero, and a `416` (range already satisfied) finalizes the part —
    /// the died-between-download-end-and-rename edge.
    /// Verifying the finalized bytes is the caller's duty: `down` digest-checks
    /// against the sibling, the `get` primitive trusts the part.
    /// The digest covers the whole file, prefix included.
    /// Returns the final file size and digest.
    pub async fn download_resumable(
        &self,
        subject: (&str, Dir),
        url: &str,
        part: &Path,
        cont: bool,
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
                if cont {
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
                    req = req.header(reqwest::header::RANGE, format!("bytes={prefix}-"));
                }
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status().as_u16();
                if status == 416 && prefix > 0 {
                    // The server refuses the range because it is already
                    // satisfied: the part holds the whole object. This is the
                    // crash-between-download-end-and-rename edge — finalize
                    // the part and let the caller digest-check and rename.
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
                let mut file = if append {
                    tokio::fs::OpenOptions::new().append(true).open(&part).await
                } else {
                    tokio::fs::File::create(&part).await
                }
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

    /// PUT the bytes of a file: streamed body with Content-Length, stall
    /// detection through a bounded channel (a full channel means the network
    /// stopped consuming).
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
                                match tokio::time::timeout(stall, tx.send(Ok(buf))).await {
                                    Ok(Ok(())) => {}
                                    // Receiver gone: the attempt was cancelled.
                                    Ok(Err(_)) => break,
                                    Err(_) => {
                                        let _ = tx
                                            .send(Err(std::io::Error::other(format!(
                                                "stalled: upload channel full for {}s",
                                                stall.as_secs_f64()
                                            ))))
                                            .await;
                                        break;
                                    }
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
                let body = reqwest::Body::wrap_stream(futures_util::stream::poll_fn(move |cx| {
                    use futures_util::task::Poll;
                    match rx.poll_recv(cx) {
                        Poll::Ready(Some(Ok(chunk))) => {
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
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                producer.abort();
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
