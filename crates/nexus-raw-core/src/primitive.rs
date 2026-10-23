//! L0 primitives: curl-grade single-object operations (spec §5.1).
//!
//! Retry, stall detection, TLS and auth come from the transport.
//! Primitives never classify, never diff and never refuse to overwrite:
//! that is the transfer layer's job.

use crate::transport::client::NexusClient;

// Everything below except `head` touches the local filesystem or stdout:
// neither exists on wasm32, so the fs-bound surface gates with the target.
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[cfg(not(target_arch = "wasm32"))]
use futures_util::StreamExt;
#[cfg(not(target_arch = "wasm32"))]
use sha2::{Digest as _, Sha256};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::AsyncWriteExt;

use crate::error::Error;
#[cfg(not(target_arch = "wasm32"))]
use crate::events::Dir;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::digest::{self, Digest};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::name::{percent_decode, ArtifactName};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::sibling;
pub use crate::transport::client::HeadInfo;

/// The ceiling on `get --segments`: enough to saturate a link, small enough
/// that the span vector and the staging directory stay bounded.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const MAX_SEGMENTS: usize = 64;

/// The `get` result.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(not(target_arch = "wasm32"))]
pub struct GetOutcome {
    /// Final size in bytes (stdout mode: bytes written).
    pub size: u64,
    /// The digest, when a file was produced (stdout streaming does not hash).
    pub digest: Option<Digest>,
    /// The offset the transfer resumed from (0 for a fresh download).
    pub resumed_from: u64,
}

/// One byte range: the inclusive start, the optional inclusive end, and the bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(not(target_arch = "wasm32"))]
pub struct RangeOutcome {
    pub start: u64,
    pub end: Option<u64>,
    pub bytes: Vec<u8>,
}

/// GET a URL.
///
/// With `out`: stream into `<out>.part` (resuming from it when `cont`), verify nothing, then rename to `out`.
/// Without `out`: stream to stdout in a single body attempt, because a retry after the body started would duplicate bytes.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the GET, [`Error::Io`] when the part file cannot be written or renamed, and [`Error::Misuse`] when stdout cannot be written.
#[cfg(not(target_arch = "wasm32"))]
pub async fn get(
    client: &NexusClient,
    url: &str,
    out: Option<PathBuf>,
    cont: bool,
) -> Result<GetOutcome, Error> {
    if let Some(out) = out {
        let part = part_of(&out);
        let resumed_from = if cont {
            tokio::fs::symlink_metadata(&part)
                .await
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map_or(0, |m| m.len())
        } else {
            0
        };
        let name = out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A failed attempt leaves the part in place: it is the resume state of
        // the rerun, and a stale or corrupt part is detected on the next run
        // (prefix re-read, 416 check, sibling digest). Deleting it here made
        // `get --continue` throw away every byte a transport failure had
        // already landed, exactly when the resume promise matters most.
        // `cont`, not `resumed_from > 0`: an attempt that cut mid-body creates
        // the part, and the retry inside this call must resume it. The client
        // refuses a symlink at the path on its own.
        let (size, digest) = client
            .download_resumable((&name, Dir::Down), url, &part, cont, None)
            .await?;
        // A `--continue` run may have composed a prefix with a tail across
        // attempts (the first response cut, the retry resumed), so the guard
        // runs whenever continue was asked, not only when a part existed
        // before the call. When a `.sha256` sibling exists (locally or on the
        // server), the finished file is checked against it before the rename,
        // and a divergence is a data error (exit 1), not a warning.
        if cont {
            let mut sibling = out.clone().into_os_string();
            sibling.push(".sha256");
            let expected = if tokio::fs::try_exists(&sibling).await.unwrap_or(false) {
                tokio::fs::read_to_string(&sibling)
                    .await
                    .ok()
                    .and_then(|t| digest_hex(&t))
            } else {
                let remote = format!("{url}.sha256");
                match client.get_small(&remote).await {
                    Ok(Some(bytes)) => String::from_utf8(bytes).ok().and_then(|t| digest_hex(&t)),
                    _ => None,
                }
            };
            if let Some(expected) = expected {
                if digest != expected {
                    tokio::fs::remove_file(&part).await.ok();
                    return Err(Error::Mismatch {
                        name: out.display().to_string(),
                        detail: format!(
                            "resumed content diverges: the .sha256 sibling says {expected}, the bytes hash {digest}"
                        ),
                    });
                }
            }
        }
        tokio::fs::rename(&part, &out)
            .await
            .map_err(|e| Error::io(&out, e))?;
        Ok(GetOutcome {
            size,
            digest: Some(digest),
            resumed_from,
        })
    } else {
        let resp = client.get_stream(url).await?;
        // No progress events in stdout mode: anything the renderer prints would corrupt the body.
        let mut body = resp.bytes_stream();
        let mut stdout = tokio::io::stdout();
        let mut size: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| Error::transport(url, e))?;
            stdout.write_all(&chunk).await.map_err(|e| {
                // A closed pipe (`nxr get URL | head`) is an I/O condition, not a command-line mistake.
                Error::Io {
                    path: "stdout".into(),
                    detail: e.to_string(),
                }
            })?;
            size += chunk.len() as u64;
        }
        stdout.flush().await.map_err(|e| Error::Io {
            path: "stdout".into(),
            detail: e.to_string(),
        })?;
        Ok(GetOutcome {
            size,
            digest: None,
            resumed_from: 0,
        })
    }
}

/// PUT a file, optionally with its sha-sibling marker (spec §5.1).
///
/// `--sha` hashes the file (one extra local pass) and PUTs `<url>.sha256` right after the bytes.
/// Returns `(bytes sent, digest when --sha)`.
///
/// # Errors
///
/// Returns [`Error::Io`] when `src` cannot be read or hashed, [`Error::Misuse`] when `src` is not a file, and transport, auth or HTTP errors from the uploads.
#[cfg(not(target_arch = "wasm32"))]
pub async fn put(
    client: &NexusClient,
    url: &str,
    src: &Path,
    sha: bool,
) -> Result<(u64, Option<Digest>), Error> {
    let meta = tokio::fs::metadata(src)
        .await
        .map_err(|e| Error::io(src, e))?;
    if !meta.is_file() {
        return Err(Error::misuse(format!("not a file: {}", src.display())));
    }
    let size = meta.len();
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // The digest rides the upload: the bytes are hashed as they hit the wire,
    // no second pass over the file and no synchronous read on the runtime.
    let d = client.upload_file((&name, Dir::Up), url, src, size).await?;
    if !sha {
        return Ok((size, None));
    }
    // The sibling URL is the object URL + ".sha256".
    // The marker names the object by its final path segment.
    let marker_name = last_segment(url);
    // An unparseable segment would land a permanently Broken marker behind exit 0.
    if let Err(e) = ArtifactName::parse(&marker_name) {
        return Err(Error::UnsafeName {
            name: marker_name,
            reason: format!("the final URL segment cannot carry a sha marker: {e}"),
        });
    }
    let marker = sibling::format_line(&marker_name, &d);
    let sib_url = format!("{url}.sha256");
    client.put_small(&sib_url, marker.into_bytes()).await?;
    Ok((size, Some(d)))
}

/// HEAD a URL: status, size, content type. 404 is a normal result.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors after the retry loop is exhausted.
pub async fn head(client: &NexusClient, url: &str) -> Result<HeadInfo, Error> {
    client.head_info(url).await
}

/// The digest of a local file or of a remote object.
#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
pub enum ShaSource {
    File(PathBuf),
    Url(String),
}

/// Stream a source through sha256 (spec §5.1 `sha <file|url>`).
///
/// # Errors
///
/// Returns [`Error::Io`] when a file source cannot be read and transport, auth or HTTP errors for a URL source.
#[cfg(not(target_arch = "wasm32"))]
pub async fn sha(client: &NexusClient, src: ShaSource) -> Result<Digest, Error> {
    match src {
        ShaSource::File(path) => digest::sha256_file(&path).map_err(|e| Error::io(&path, e)),
        ShaSource::Url(url) => {
            let resp = client.get_stream(&url).await?;
            let mut body = resp.bytes_stream();
            let mut hasher = Sha256::new();
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(|e| Error::transport(&url, e))?;
                hasher.update(&chunk);
            }
            Ok(Digest::from_hex_string(digest::hex(&hasher.finalize())))
        }
    }
}

/// GET a URL in `segments` concurrent byte-range pieces (spec §5.1 `get --segments`).
///
/// A HEAD gives the size, the span splits into `segments` contiguous closed ranges,
/// and each range streams into its own staging file under `dir` (retries included).
/// A server that answers a ranged GET with the whole body (`Range` ignored) fails one
/// segment with [`Error::RangeIgnored`]: the caller answers it with the plain single-stream path.
/// The staging files assemble positionally into `<out>.part`, one read pass hashes the result,
/// a `.sha256` sibling (local first, then remote) verifies it when present, and the rename to
/// `out` is the usual atomic completion.
/// Nothing partial is ever renamed: a failure leaves the staging files for inspection.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from any segment, [`Error::Io`] when staging, assembly or the rename fail, and [`Error::Mismatch`] when a sibling digest diverges.
#[cfg(not(target_arch = "wasm32"))]
pub async fn get_segmented(
    client: &NexusClient,
    url: &str,
    out: &Path,
    segments: usize,
    events: &crate::events::Progress,
) -> Result<GetOutcome, Error> {
    // The ceiling lives where the fan-out is built: the facade and external
    // callers of this primitive share one bound.
    if !(2..=MAX_SEGMENTS).contains(&segments) {
        return Err(Error::misuse(format!(
            "segments must be between 2 and {MAX_SEGMENTS}, got {segments}"
        )));
    }
    // The name carries progress events, the URL does not: one label for the whole fan-out.
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let info = client.head_info(url).await?;
    if !(200..300).contains(&info.status) {
        return Err(Error::Http {
            status: info.status,
            url: url.to_owned(),
        });
    }
    let Some(total) = info.size else {
        return Err(Error::transport(
            url,
            "the HEAD answer carries no Content-Length: the object cannot be split into ranges",
        ));
    };
    let part = part_of(out);
    // An empty object has no span to split: the part is born complete, and
    // `total / segments` below would divide by zero.
    if total == 0 {
        // write_options, not fs::write: the part path is predictable and must
        // never follow a symlink planted beside the output.
        let mut empty = crate::transport::client::write_options(false)
            .open(&part)
            .await
            .map_err(|e| Error::io(&part, e))?;
        empty.flush().await.map_err(|e| Error::io(&part, e))?;
        let digest = crate::model::digest::sha256_file(&part).map_err(|e| Error::io(&part, e))?;
        verify_sibling(client, url, out, &part, &digest).await?;
        tokio::fs::rename(&part, out)
            .await
            .map_err(|e| Error::io(out, e))?;
        events.done(&name, Dir::Down, false, 0, Some(0)).await;
        return Ok(GetOutcome {
            size: 0,
            digest: Some(digest),
            resumed_from: 0,
        });
    }
    // Every segment spans at least one byte. Spans are `total / segments` sized with
    // the remainder spread over the leading segments.
    let base = total / segments as u64;
    let segments = if base == 0 {
        // The object is smaller than the segment count: one segment per byte,
        // computed iteratively (a recursive degrade re-entered this huge frame
        // on the same stack and overflowed the default windows thread stack).
        total as usize
    } else {
        segments
    };
    let base = total / segments as u64;
    let rem = total % segments as u64;
    let staging = temp_staging_dir(out).await?;
    let mut bounds: Vec<(u64, u64)> = Vec::with_capacity(segments);
    let mut start: u64 = 0;
    for i in 0..segments {
        let len = base + u64::from(i < rem as usize);
        bounds.push((start, start + len - 1));
        start += len;
    }
    debug_assert_eq!(start, total, "the spans must tile the object exactly");

    // Each segment streams into its own staging file, concurrently, retries included.
    // The worker ceiling still applies: `--segments` splits the object, it does
    // not widen the concurrency budget the rest of the tool obeys.
    let workers = client.workers();
    let fetches: Vec<_> = bounds
        .iter()
        .enumerate()
        .map(|(i, &(start, end_incl))| {
            let file = staging.join(format!("segment-{i:04}.part"));
            let url = url.to_owned();
            let name = name.clone();
            let workers = workers.clone();
            async move {
                let _permit = workers
                    .acquire_owned()
                    .await
                    .expect("the worker semaphore is never closed");
                let done = client
                    .fetch_range((&name, Dir::Down), &url, start, end_incl, &file)
                    .await
                    .map_err(|e| (i, e))?;
                Ok::<(usize, u64), (usize, Error)>((i, done))
            }
        })
        .collect();
    let staged: Vec<Result<(usize, u64), (usize, Error)>> =
        futures_util::future::join_all(fetches).await;
    // Any segment failure fails the whole get: the staging files stay for inspection.
    // The per-segment counts rode the progress events already, nothing to aggregate.
    for r in staged {
        match r {
            Ok(_) => {}
            Err((_, e)) => {
                if matches!(e, Error::RangeIgnored { .. }) {
                    // The whole-body answer to a ranged GET: the caller falls back
                    // to the single-stream path. The staging dir is empty here
                    // (the 200 arrived before any byte was written), so it cleans up.
                    let _ = tokio::fs::remove_dir_all(&staging).await;
                    return Err(e);
                }
                return Err(e);
            }
        }
    }

    // Positional assembly into the part file: each segment streams in at its offset.
    assemble(&bounds, &staging, &part, total).await?;
    // One read pass over the assembled file: the digest, then the sibling verdict.
    let digest = crate::model::digest::sha256_file(&part).map_err(|e| Error::io(&part, e))?;
    verify_sibling(client, url, out, &part, &digest).await?;
    tokio::fs::rename(&part, out)
        .await
        .map_err(|e| Error::io(out, e))?;
    // The staging area only outlives a failed run.
    let _ = tokio::fs::remove_dir_all(&staging).await;
    events
        .done(&name, Dir::Down, false, total, Some(total))
        .await;
    Ok(GetOutcome {
        size: total,
        digest: Some(digest),
        resumed_from: 0,
    })
}

/// Assemble the staged segments positionally into `part`: every segment file opens,
/// seeks to its start offset and streams its bytes in one pass, no full-file buffer.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn assemble(
    bounds: &[(u64, u64)],
    staging: &Path,
    part: &Path,
    total: u64,
) -> Result<(), Error> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    let mut out = crate::transport::client::write_options(false)
        .open(part)
        .await
        .map_err(|e| Error::io(part, e))?;
    out.set_len(total).await.map_err(|e| Error::io(part, e))?;
    for (i, &(start, _)) in bounds.iter().enumerate() {
        let file = staging.join(format!("segment-{i:04}.part"));
        let mut seg = tokio::fs::File::open(&file)
            .await
            .map_err(|e| Error::io(&file, e))?;
        out.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| Error::io(part, e))?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = seg.read(&mut buf).await.map_err(|e| Error::io(&file, e))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])
                .await
                .map_err(|e| Error::io(part, e))?;
        }
    }
    out.flush().await.map_err(|e| Error::io(part, e))?;
    Ok(())
}

/// The completion digest check of the segmented path: a local `.sha256` sibling wins,
/// the remote one answers when the local is absent, and a divergence is a data error
/// (exit 1) that also removes the part, so nothing partial is ever renamed.
#[cfg(not(target_arch = "wasm32"))]
async fn verify_sibling(
    client: &NexusClient,
    url: &str,
    out: &Path,
    part: &Path,
    digest: &Digest,
) -> Result<(), Error> {
    let mut sibling = out.as_os_str().to_os_string();
    sibling.push(".sha256");
    let sibling = PathBuf::from(sibling);
    let expected = if tokio::fs::try_exists(&sibling).await.unwrap_or(false) {
        tokio::fs::read_to_string(&sibling)
            .await
            .ok()
            .and_then(|t| digest_hex(&t))
    } else {
        let remote = format!("{url}.sha256");
        match client.get_small(&remote).await {
            Ok(Some(bytes)) => String::from_utf8(bytes).ok().and_then(|t| digest_hex(&t)),
            _ => None,
        }
    };
    if let Some(expected) = expected {
        if *digest != expected {
            tokio::fs::remove_file(part).await.ok();
            return Err(Error::Mismatch {
                name: out.display().to_string(),
                detail: format!(
                    "segmented content diverges: the .sha256 sibling says {expected}, the bytes hash {digest}"
                ),
            });
        }
    }
    Ok(())
}

/// The staging directory for a segmented get: `<out>.segments/` beside the output.
/// Created on demand. A failure leaves its files for inspection.
#[cfg(not(target_arch = "wasm32"))]
async fn temp_staging_dir(out: &Path) -> Result<PathBuf, Error> {
    let mut s = out.as_os_str().to_os_string();
    s.push(".segments");
    let dir = PathBuf::from(s);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::io(&dir, e))?;
    Ok(dir)
}

/// `<out>.part` next to the output file.
#[cfg(not(target_arch = "wasm32"))]
fn part_of(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// The sha256 sum inside a marker-style text (`<hex>` or `<hex>  <name>`).
#[cfg(not(target_arch = "wasm32"))]
fn digest_hex(text: &str) -> Option<Digest> {
    let hex = text.split_whitespace().next()?;
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(Digest::from_hex_string(hex.to_lowercase()))
    } else {
        None
    }
}

/// The final path segment of a URL, percent-decoded: the marker's name field.
#[cfg(not(target_arch = "wasm32"))]
fn last_segment(url: &str) -> String {
    let raw = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.path().rsplit('/').next().map(str::to_owned))
        .unwrap_or_default();
    percent_decode(&raw)
}

/// Parse `START-END` or `START-` into an inclusive start and an optional end.
pub fn parse_range(spec: &str) -> Result<(u64, Option<u64>), Error> {
    let (start, end) = spec
        .split_once('-')
        .ok_or_else(|| Error::Misuse("range expects START-END or START-".to_owned()))?;
    if start.is_empty() {
        return Err(Error::Misuse(
            "range does not take a suffix (-N): use START- or START-END".to_owned(),
        ));
    }
    let start = start
        .parse::<u64>()
        .map_err(|_| Error::Misuse(format!("range start is not a number: {start}")))?;
    if end.is_empty() {
        return Ok((start, None));
    }
    let end = end
        .parse::<u64>()
        .map_err(|_| Error::Misuse(format!("range end is not a number: {end}")))?;
    if end < start {
        return Err(Error::Misuse("range end is before its start".to_owned()));
    }
    Ok((start, Some(end)))
}

/// Write bytes beside `path` through an exclusive temporary, then rename.
///
/// A symlink planted at the output is replaced, never followed.
/// The `.nxr` prefix keeps the temporary out of every scan.
///
/// # Errors
///
/// Returns [`Error::Io`] when the temporary cannot be created, written or renamed.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    for n in 0..100u32 {
        let candidate = parent.join(format!(".nxr-range-{}-{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                let written = file.write_all(bytes).and_then(|()| file.flush());
                drop(file);
                if let Err(e) = written {
                    let _ = std::fs::remove_file(&candidate);
                    return Err(Error::Io {
                        path: candidate.display().to_string(),
                        detail: e.to_string(),
                    });
                }
                std::fs::rename(&candidate, path).map_err(|e| {
                    let _ = std::fs::remove_file(&candidate);
                    Error::Io {
                        path: path.display().to_string(),
                        detail: e.to_string(),
                    }
                })?;
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(Error::Io {
                    path: candidate.display().to_string(),
                    detail: e.to_string(),
                });
            }
        }
    }
    Err(Error::Io {
        path: path.display().to_string(),
        detail: "cannot create a temporary file beside the output".to_owned(),
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn part_naming() {
        assert_eq!(
            part_of(Path::new("d/app.zip")),
            PathBuf::from("d/app.zip.part")
        );
    }

    #[test]
    fn url_last_segment_decodes() {
        assert_eq!(last_segment("http://h/a/b/app.zip"), "app.zip");
        assert_eq!(last_segment("http://h/a/b/my%20app.zip"), "my app.zip");
    }

    #[test]
    fn parses_the_range_forms() {
        assert_eq!(parse_range("10-19").unwrap(), (10, Some(19)));
        assert_eq!(parse_range("10-").unwrap(), (10, None));
        assert_eq!(parse_range("0-0").unwrap(), (0, Some(0)));
    }

    #[test]
    fn refuses_malformed_ranges() {
        for spec in ["", "10", "-5", "19-10", "x-2", "1-x", "1-2-3", " 1-2"] {
            assert!(parse_range(spec).is_err(), "{spec:?} must be refused");
        }
    }
}
