//! `NexusClient`: GET/HEAD/PUT over reqwest; auth, TLS, retries, stall detection (protocol §7).

use std::future::Future;
use std::path::Path;
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
use crate::model::name::{self, ArtifactName};
use crate::model::pointer;
use crate::model::sibling;
use crate::model::state::RemoteStatus;
use crate::transport::retry::{is_retryable, AttemptFailure, RetryPolicy};

/// Outgoing body chunk size: small enough that a full outbound buffer drains
/// well inside the stall timeout, so channel backpressure means a real stall.
const UPLOAD_CHUNK: usize = 16 * 1024;
/// Depth of the outgoing body channel.
const UPLOAD_CHANNEL: usize = 4;

type Attempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, AttemptFailure>> + Send + 'a>>;

pub struct NexusClient {
    http: reqwest::Client,
    base: String,
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
            base: Config::normalized_base(&cfg.base)?,
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

    pub fn object_url(&self, version: &str, name: &ArtifactName) -> String {
        format!("{base}{version}/{}", name.encoded(), base = self.base)
    }

    pub fn claim_url(&self, version: &str) -> String {
        format!("{base}{version}/claim.json", base = self.base)
    }

    pub fn pointer_url(&self, pointer: &str) -> String {
        format!("{base}{pointer}", base = self.base)
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
                    "HTTP {status}; set NXR_AUTH or NXR_<PROFILE>_AUTH (base64 user:pass)"
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
        self.with_retries(None, url, |this: &Self, url: &str| {
            Box::pin(async move {
                let req = this.authorize(this.http.head(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                match status {
                    s if s.is_success() => {
                        let size = resp
                            .headers()
                            .get(reqwest::header::CONTENT_LENGTH)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok());
                        Ok(size)
                    }
                    s if s.as_u16() == 404 => Ok(None),
                    s => Err(this.status_failure(url, s)),
                }
            })
        })
        .await
    }

    /// GET a small object (sibling, claim, pointer): None on 404.
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
    pub async fn probe(&self, version: &str, name: &ArtifactName) -> Result<RemoteStatus, Error> {
        let bytes_url = self.object_url(version, name);
        let Some(size) = self.head(&bytes_url).await? else {
            return Ok(RemoteStatus::Absent);
        };
        let sib_url = format!("{bytes_url}.sha256");
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

    /// GET bytes into a temp file: hash on the fly, per-chunk stall detection,
    /// retries restart the attempt from scratch. Returns (bytes read, digest).
    pub async fn download_to_file(
        &self,
        subject: (&str, Dir),
        url: &str,
        dest: &Path,
        total_hint: Option<u64>,
    ) -> Result<(u64, Digest), Error> {
        let stall = self.stall;
        let progress = self.events.clone();
        let subject_name = subject.0.to_owned();
        self.with_retries(Some(subject), url, |this: &Self, url: &str| {
            let dest = dest.to_owned();
            let progress = progress.clone();
            let subject_name = subject_name.clone();
            Box::pin(async move {
                let req = this.authorize(this.http.get(url));
                let resp = req.send().await.map_err(|e| this.wrap_send_err(url, e))?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(this.status_failure(url, status));
                }
                let total = resp.content_length().or(total_hint);
                progress.started(&subject_name, Dir::Down, total).await;
                let mut body = resp.bytes_stream();
                let mut file =
                    tokio::fs::File::create(&dest)
                        .await
                        .map_err(|e| AttemptFailure {
                            retryable: false,
                            error: Error::io(&dest, e),
                        })?;
                let mut hasher = Sha256::new();
                let mut done: u64 = 0;
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
                        error: Error::io(&dest, e),
                    })?;
                    done += chunk.len() as u64;
                    progress.bytes(&subject_name, Dir::Down, done, total).await;
                }
                file.flush().await.map_err(|e| AttemptFailure {
                    retryable: false,
                    error: Error::io(&dest, e),
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

    /// Read a pointer: None on 404, raw bytes otherwise.
    pub async fn get_pointer(&self, pointer_name: &str) -> Result<Option<String>, Error> {
        let url = self.pointer_url(pointer_name);
        self.get_small(&url)
            .await
            .map(|opt| opt.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    /// Atomic pointer PUT: `<token>\n`.
    pub async fn put_pointer(&self, pointer_name: &str, token: &str) -> Result<(), Error> {
        let url = self.pointer_url(pointer_name);
        self.put_small(&url, pointer::format_token(token).into_bytes())
            .await
    }

    /// Read the remote claim: None on 404; a broken claim is ClaimDrift (§4.1 sane check).
    pub async fn get_claim(
        &self,
        version: &str,
    ) -> Result<Option<crate::model::claim::Claim>, Error> {
        let url = self.claim_url(version);
        match self.get_small(&url).await? {
            None => Ok(None),
            Some(raw) => crate::model::claim::Claim::from_slice(&raw)
                .map(Some)
                .map_err(|e| Error::ClaimDrift {
                    version: version.to_owned(),
                    detail: format!("remote claim.json does not parse: {e}"),
                }),
        }
    }

    /// PUT the claim with a drift check: present and byte-equal — ok, different — refuse (§6.1).
    pub async fn put_claim_checked(&self, version: &str, claim_bytes: &[u8]) -> Result<(), Error> {
        let url = self.claim_url(version);
        match self.get_small(&url).await? {
            Some(existing) if existing == claim_bytes => Ok(()),
            Some(_) => Err(Error::ClaimDrift {
                version: version.to_owned(),
                detail: "remote claim.json differs from the local one; claims are immutable"
                    .to_owned(),
            }),
            None => self.put_small(&url, claim_bytes.to_owned()).await,
        }
    }

    /// Version list via REST search (experimental; endpoint depends on the Nexus release).
    ///
    /// base `…/repository/<repo>/<group…>/` → search the assets of repository
    /// `repo` with group `<group…>`; the version is the path segment right
    /// after the group prefix.
    pub async fn search_versions(&self) -> Result<Vec<String>, Error> {
        let url =
            reqwest::Url::parse(&self.base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
        let mut segments = url.path().split('/').filter(|s| !s.is_empty());
        let repo = match segments.next() {
            Some("repository") => segments.next(),
            _ => None,
        };
        let Some(repo) = repo else {
            return Err(Error::misuse(
                "base URL must point inside /repository/<name>/ for ls without --version",
            ));
        };
        let group: Vec<&str> = segments.collect();
        let mut origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
        if let Some(port) = url.port() {
            origin.push_str(&format!(":{port}"));
        }
        let mut endpoint = reqwest::Url::parse(&format!("{origin}/service/rest/v1/search/assets"))
            .map_err(|e| Error::misuse(format!("search endpoint: {e}")))?;
        endpoint.query_pairs_mut().append_pair("repository", repo);
        if !group.is_empty() {
            endpoint
                .query_pairs_mut()
                .append_pair("group", &group.join("/"));
        }
        let mut versions = std::collections::BTreeSet::new();
        let mut next: Option<String> = None;
        loop {
            let mut page = endpoint.clone();
            if let Some(token) = &next {
                page.query_pairs_mut()
                    .append_pair("continuationToken", token);
            }
            let page_url = page.to_string();
            let bytes = self
                .get_small(&page_url)
                .await?
                .ok_or_else(|| Error::Http {
                    status: 404,
                    url: page_url.clone(),
                })?;
            let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
                Error::transport(&page_url, format!("search response is not JSON: {e}"))
            })?;
            if let Some(items) = value.get("items").and_then(|i| i.as_array()) {
                for item in items {
                    let Some(path) = item.get("path").and_then(|p| p.as_str()) else {
                        continue;
                    };
                    let rel = strip_group_prefix(path, &group);
                    if let Some(vseg) = rel.split('/').next() {
                        if name::validate_version(vseg).is_ok() {
                            versions.insert(vseg.to_owned());
                        }
                    }
                }
            }
            next = value
                .get("continuationToken")
                .and_then(|t| t.as_str())
                .map(str::to_owned);
            if next.is_none() {
                break;
            }
        }
        Ok(versions.into_iter().collect())
    }
}

fn strip_group_prefix<'a>(path: &'a str, group: &[&str]) -> &'a str {
    if group.is_empty() {
        return path;
    }
    let prefix = format!("{}/", group.join("/"));
    path.strip_prefix(&prefix).unwrap_or(path)
}
