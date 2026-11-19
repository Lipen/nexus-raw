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
use crate::events::{Dir, Event};
#[cfg(not(target_arch = "wasm32"))]
use crate::model::digest;
use crate::model::digest::Digest;
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

/// A streaming byte span: the resolved bounds and what landed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(not(target_arch = "wasm32"))]
pub struct SpanOutcome {
    pub start: u64,
    pub end: Option<u64>,
    /// Bytes written for the span, not the full object size.
    pub size: u64,
    /// Span bytes that were already on disk and reused (0 for a fresh span).
    pub resumed_from: u64,
}

/// GET a URL.
///
/// With `out`: stream into `<out>.part` (resuming from it when `cont`), verify nothing, then rename to `out`.
/// A `<out>.part` parked by a ranged download resumes that span, recorded bounds and all, and a span has no object digest to verify.
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
    no_sha: bool,
) -> Result<GetOutcome, Error> {
    if let Some(out) = out {
        if cont {
            let part = part_of(&out);
            if let Some(record) = read_span_record(&part)? {
                if part_len(&part)? > 0 {
                    let record = reconcile_span_record(&part, url, record.start, record.end)?;
                    let probe = probe_span(client, url, record.start, record.end).await?;
                    let span = get_span_to_file(
                        client,
                        url,
                        record.start,
                        record.end,
                        &out,
                        true,
                        probe,
                        record,
                    )
                    .await?;
                    return Ok(GetOutcome {
                        size: span.size,
                        digest: None,
                        resumed_from: span.resumed_from,
                    });
                }
            }
            // A record without its part is stale state.
            clear_span_record(&out);
        } else {
            // A fresh whole-object run discards a parked span, part included:
            // the tail bytes cannot be a whole-object prefix.
            clear_parked_span(&out)?;
        }
        let part = part_of(&out);
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
        let (resumed_from, size, digest) = client
            .download_resumable((&name, Dir::Down), url, &part, cont, None)
            .await?;
        // A `--continue` run may have composed a prefix with a tail across
        // attempts (the first response cut, the retry resumed), so the guard
        // runs whenever continue was asked, not only when a part existed
        // before the call. When a `.sha256` sibling exists (locally or on the
        // server), the finished file is checked against it before the rename,
        // and a divergence is a data error (exit 1), not a warning.
        // A marker that does not parse is a refusal as well: an unreadable
        // digest verifies nothing, so skipping the check would bless the bytes.
        // Every complete object written to a file is verified, a plain fetch and a
        // resume alike; `--no-sha` is the opt-out, and a missing sibling skips the check.
        if !no_sha {
            let what = if cont { "resumed" } else { "fetched" };
            verify_sibling(client, url, &out, &part, &digest, what).await?;
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

/// GET one byte span as a stream (spec §5.1 `get --range`).
///
/// A file target streams into `<out>.part` exactly like a whole-object download:
/// the part grows while the transfer runs, and `--continue` resumes it through
/// `Range: bytes=(start + part_len)-`.
/// The span the part belongs to is recorded beside it, so a later
/// `--continue` needs no repeated `--range`.
/// A stdout target streams the cut span straight out, with no retry after the
/// first byte (a retry would duplicate bytes on the stream).
/// Progress events carry absolute object positions and the full object length;
/// an open-ended span learns both from a HEAD, which also names a probe failure
/// (a status, a missing size, or a start past the end).
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the HEAD or GET, [`Error::Io`]
/// when the part cannot be written or renamed, and [`Error::Misuse`] for a
/// malformed range or a start past the object's end.
#[cfg(not(target_arch = "wasm32"))]
pub async fn get_span(
    client: &NexusClient,
    url: &str,
    spec: &str,
    out: Option<PathBuf>,
    cont: bool,
) -> Result<SpanOutcome, Error> {
    let (start, end, probe) = resolve_span(client, url, spec).await?;
    match out {
        Some(out) => {
            let part = part_of(&out);
            let record = if cont {
                reconcile_span_record(&part, url, start, end)?
            } else {
                // A fresh span owns the part: bytes recorded for another span
                // or another URL, or unrecorded bytes, cannot be its prefix.
                // The same span of the same URL is kept until the first answer
                // truncates it, so a failed fresh retry still resumes cleanly.
                let recorded = read_span_record(&part)?;
                let requested_url = resume_url(url);
                let same = recorded.as_ref().is_some_and(|r| {
                    (r.start, r.end) == (start, end)
                        && r.url.as_deref().is_none_or(|u| u == requested_url)
                });
                if !same {
                    let _ = std::fs::remove_file(&part);
                }
                SpanRecord {
                    url: Some(requested_url),
                    start,
                    end,
                    total: None,
                    etag: None,
                    last_modified: None,
                }
            };
            get_span_to_file(client, url, start, end, &out, cont, probe, record).await
        }
        None => {
            let size = client.stream_span(url, start, end, probe.total).await?;
            Ok(SpanOutcome {
                start,
                end,
                size,
                resumed_from: 0,
            })
        }
    }
}

/// The file half of [`get_span`]: fill the record, stream into the part, rename,
/// and drop the record.
/// The bounds and the part state are already reconciled by the caller.
#[allow(clippy::too_many_arguments)]
#[cfg(not(target_arch = "wasm32"))]
async fn get_span_to_file(
    client: &NexusClient,
    url: &str,
    start: u64,
    end: Option<u64>,
    out: &Path,
    cont: bool,
    probe: SpanProbe,
    mut record: SpanRecord,
) -> Result<SpanOutcome, Error> {
    let part = part_of(out);
    let old_total = record.total;
    let old_strong = strong_etag(record.etag.as_deref()).map(str::to_owned);
    let had_prefix = part_len(&part)? > 0;
    // The probe describes the object now. Compare it with what the part was
    // written against before the record is overwritten, or the new validator
    // would match its own object and splice the old bytes silently.
    let mut restart = false;
    if cont && had_prefix {
        if let (Some(old), Some(new)) = (old_total, probe.total) {
            if old != new {
                restart = true;
            }
        }
        if let (Some(old), Some(new)) = (old_strong.as_deref(), strong_etag(probe.etag.as_deref()))
        {
            if old != new {
                restart = true;
            }
        }
    }
    if restart {
        let _ = std::fs::remove_file(&part);
        let _ = client.events().sender().send(Event::Warning {
            text: "the object changed since the part was written: the span restarts from its beginning"
                .to_owned(),
        });
    }
    record.url = Some(resume_url(url));
    record.start = start;
    record.end = end;
    // With a parked prefix still in play, the probe describes the object now,
    // not the bytes already on disk. Writing its validator into the record
    // would bless those old bytes for the next resume. Keep the parked values
    // until the part is discarded or the transfer completes.
    if !had_prefix || restart {
        if probe.total.is_some() {
            record.total = probe.total;
        }
        if probe.etag.is_some() {
            record.etag = probe.etag;
        }
        if probe.last_modified.is_some() {
            record.last_modified = probe.last_modified;
        }
    }
    write_span_record(&part, &record)?;
    // A fresh run has nothing to validate, a restart just discarded the part,
    // and a weak ETag cannot prove the object unchanged inside the same second:
    // only the parked strong ETag goes out.
    let if_range = if cont && !restart && had_prefix {
        old_strong.as_deref()
    } else {
        None
    };
    let resumed = cont && !restart;
    let name = out
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (resumed_from, size, _) = client
        .download_span(
            (&name, Dir::Down),
            url,
            start,
            end,
            &part,
            resumed,
            record.total,
            if_range,
        )
        .await?;
    tokio::fs::rename(&part, out)
        .await
        .map_err(|e| Error::io(out, e))?;
    let _ = std::fs::remove_file(span_record_path(&part));
    Ok(SpanOutcome {
        start,
        end,
        size,
        resumed_from,
    })
}

/// The span a parked `<out>.part` belongs to, when a prior ranged run recorded
/// it. `None` when no part exists or the part carries no span record (a whole
/// object download).
///
/// # Errors
///
/// Returns [`Error::Io`] when the part or its record cannot be read, and a
/// record outside `START END` is refused rather than misread as a whole part.
#[cfg(not(target_arch = "wasm32"))]
pub fn recorded_span(out: &Path) -> Result<Option<(u64, Option<u64>)>, Error> {
    let part = part_of(out);
    match std::fs::symlink_metadata(&part) {
        Ok(meta) if meta.is_file() => Ok(read_span_record(&part)?.map(|r| (r.start, r.end))),
        Ok(_) => Err(Error::Io {
            path: part.display().to_string(),
            detail: "the part path is not a regular file".to_owned(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(&part, e)),
    }
}

/// Drop a parked span: its record and the tail part it describes.
/// A fresh whole-object or segmented run owns the part now.
///
/// # Errors
///
/// Returns [`Error::Io`] when the record cannot be read.
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_parked_span(out: &Path) -> Result<(), Error> {
    let part = part_of(out);
    if read_span_record(&part)?.is_some() {
        let _ = std::fs::remove_file(&part);
        clear_span_record(out);
    }
    Ok(())
}

/// Drop a parked span record: a fresh whole-object run owns the part now.
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_span_record(out: &Path) {
    let _ = std::fs::remove_file(span_record_path(&part_of(out)));
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

/// The `.sha256` marker digest of one object, when the server has a marker.
///
/// A missing marker is `None`; a marker outside the `sha256sum -c` format is
/// [`Error::Mismatch`], because a digest that cannot be read cannot verify anything.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the marker read, and
/// [`Error::Mismatch`] when the marker does not parse.
pub async fn sibling_digest(client: &NexusClient, url: &str) -> Result<Option<Digest>, Error> {
    let marker_url = format!("{url}.sha256");
    let Some(raw) = client.get_small(&marker_url).await? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&raw);
    let parsed = crate::model::sibling::parse_line(&text).map_err(|reason| Error::Mismatch {
        name: url.to_owned(),
        detail: format!("the .sha256 marker does not parse: {reason}"),
    })?;
    if parsed.name.is_none() {
        client.events().bare_sibling();
    }
    Ok(Some(parsed.digest))
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
    no_sha: bool,
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
    // The HEAD is the first request of the transfer: a 5xx retries and a 401
    // takes the auth verdict, exactly like the range GETs that follow.
    let info = client.head_transfer(url).await?;
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
        if !no_sha {
            verify_sibling(client, url, out, &part, &digest, "segmented").await?;
        }
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
    if !no_sha {
        verify_sibling(client, url, out, &part, &digest, "segmented").await?;
    }
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

/// The completion digest check of both file paths: a local `.sha256` sibling wins,
/// the remote one answers when the local is absent, and a divergence is a data error
/// (exit 1) that also removes the part, so nothing partial is ever renamed.
///
/// A sibling that exists must parse: a digest that cannot be read verifies nothing, so a
/// marker outside the `sha256sum -c` format is a refusal, never a skipped check.
/// A marker read that fails is an error too: an absent marker is `None`, a broken one is not.
///
/// # Errors
///
/// Returns [`Error::Io`] when a local sibling exists but cannot be read, [`Error::Mismatch`]
/// when a sibling does not parse or diverges from `digest`, and transport, auth or HTTP
/// errors from the remote marker read.
#[cfg(not(target_arch = "wasm32"))]
async fn verify_sibling(
    client: &NexusClient,
    url: &str,
    out: &Path,
    part: &Path,
    digest: &Digest,
    what: &str,
) -> Result<(), Error> {
    let mut sibling = out.as_os_str().to_os_string();
    sibling.push(".sha256");
    let sibling = PathBuf::from(sibling);
    let expected = if tokio::fs::try_exists(&sibling).await.unwrap_or(false) {
        let text = tokio::fs::read_to_string(&sibling)
            .await
            .map_err(|e| Error::io(&sibling, e))?;
        Some(parse_marker(&text, out, client)?)
    } else {
        sibling_digest(client, url).await?
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    if *digest != expected {
        tokio::fs::remove_file(part).await.ok();
        return Err(Error::Mismatch {
            name: out.display().to_string(),
            detail: format!(
                "{what} content diverges: the .sha256 sibling says {expected}, the bytes hash {digest}"
            ),
        });
    }
    Ok(())
}

/// The digest of a local marker file: a foreign line is a refusal, a bare checksum is read and noted.
#[cfg(not(target_arch = "wasm32"))]
fn parse_marker(text: &str, out: &Path, client: &NexusClient) -> Result<Digest, Error> {
    let parsed = crate::model::sibling::parse_line(text).map_err(|reason| Error::Mismatch {
        name: out.display().to_string(),
        detail: format!("the local .sha256 marker does not parse: {reason}"),
    })?;
    if parsed.name.is_none() {
        client.events().bare_sibling();
    }
    Ok(parsed.digest)
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

/// The span record of a parked part: a `.nxr`-prefixed hidden file beside it,
/// keyed by the part's file name. The prefix reserves the name from the
/// artifact grammar, so `up` never publishes local resume state.
#[cfg(not(target_arch = "wasm32"))]
fn span_record_path(part: &Path) -> PathBuf {
    let parent = part
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file = part.file_name().unwrap_or(part.as_os_str());
    let token = digest::hex(&Sha256::digest(file.as_encoded_bytes())[..16]);
    parent.join(format!(".nxr-span-{token}"))
}

/// The parked-span manifest beside a part.
/// A strong ETag is the only validator a resume sends through `If-Range`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SpanRecord {
    /// The object URL with userinfo, query and fragment removed.
    #[serde(default)]
    url: Option<String>,
    start: u64,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    etag: Option<String>,
    /// Stored for diagnosis, never sent as `If-Range`.
    #[serde(default)]
    last_modified: Option<String>,
}

/// The part length, 0 when it does not exist.
#[cfg(not(target_arch = "wasm32"))]
fn part_len(part: &Path) -> Result<u64, Error> {
    match std::fs::symlink_metadata(part) {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err(Error::Io {
            path: part.display().to_string(),
            detail: "the part path is not a regular file".to_owned(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(Error::io(part, e)),
    }
}

/// The URL as a resume identity: a different query, fragment or credential
/// never changes which object the part holds.
#[cfg(not(target_arch = "wasm32"))]
fn resume_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut url) => {
            url.set_query(None);
            url.set_fragment(None);
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => url.to_owned(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn bad_span_record(path: &Path) -> Error {
    Error::Io {
        path: path.display().to_string(),
        detail: "the span record does not parse: delete it and the part".to_owned(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_span_record(part: &Path) -> Result<Option<SpanRecord>, Error> {
    let path = span_record_path(part);
    let bytes = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => std::fs::read(&path).map_err(|e| Error::io(&path, e))?,
        Ok(_) => {
            return Err(Error::Io {
                path: path.display().to_string(),
                detail: "the span record is not a regular file".to_owned(),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&path, e)),
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| bad_span_record(&path))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(bad_span_record(&path));
    }
    if text.starts_with('{') {
        let record: SpanRecord = serde_json::from_str(text).map_err(|_| bad_span_record(&path))?;
        if record.end.is_some_and(|e| e < record.start) {
            return Err(bad_span_record(&path));
        }
        return Ok(Some(record));
    }
    // 0.28.0 wrote one plain `START END` line.
    let (start, end) = text.split_once(' ').ok_or_else(|| bad_span_record(&path))?;
    let start = start.parse::<u64>().map_err(|_| bad_span_record(&path))?;
    let end = match end.trim() {
        "-" => None,
        n => Some(n.parse::<u64>().map_err(|_| bad_span_record(&path))?),
    };
    if end.is_some_and(|e| e < start) {
        return Err(bad_span_record(&path));
    }
    Ok(Some(SpanRecord {
        url: None,
        start,
        end,
        total: None,
        etag: None,
        last_modified: None,
    }))
}

#[cfg(not(target_arch = "wasm32"))]
fn write_span_record(part: &Path, record: &SpanRecord) -> Result<(), Error> {
    let bytes = serde_json::to_vec(record).map_err(|e| Error::Io {
        path: span_record_path(part).display().to_string(),
        detail: format!("the span record cannot serialize: {e}"),
    })?;
    write_bytes_atomically(&span_record_path(part), &bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn span_label(start: u64, end: Option<u64>) -> String {
    match end {
        Some(e) => format!("{start}-{e}"),
        None => format!("{start}-"),
    }
}

/// A `--continue` that names a span must name the same span of the same object
/// the part holds.
/// An empty part carries no bytes to misplace, so a new span or URL may claim it.
/// A non-empty part without a record belongs to a whole-object download, and
/// appending a span after its bytes would assemble an artifact no request described.
#[cfg(not(target_arch = "wasm32"))]
fn reconcile_span_record(
    part: &Path,
    url: &str,
    start: u64,
    end: Option<u64>,
) -> Result<SpanRecord, Error> {
    let recorded = read_span_record(part)?;
    let requested_url = resume_url(url);
    let requested = SpanRecord {
        url: Some(requested_url.clone()),
        start,
        end,
        total: None,
        etag: None,
        last_modified: None,
    };
    if part_len(part)? == 0 {
        return Ok(recorded.unwrap_or(requested));
    }
    let Some(recorded) = recorded else {
        return Err(Error::Misuse(format!(
            "the part {} has no span record: rerun without --continue to start the span over, or delete the part",
            part.display()
        )));
    };
    if (recorded.start, recorded.end) != (start, end) {
        let recorded_label = span_label(recorded.start, recorded.end);
        let requested_label = span_label(start, end);
        return Err(Error::Misuse(format!(
            "the part {} holds the span {recorded_label}, not {requested_label}: pass --continue with the recorded range, or delete the part",
            part.display()
        )));
    }
    if let Some(recorded_url) = &recorded.url {
        if recorded_url != &requested_url {
            return Err(Error::Misuse(format!(
                "the part {} belongs to {recorded_url}, not {requested_url}: delete the part, or resume the original URL",
                part.display()
            )));
        }
    }
    Ok(recorded)
}

/// The strong ETag, the only validator `If-Range` accepts.
/// A weak `W/...` value can match an object rewritten inside the same second.
#[cfg(not(target_arch = "wasm32"))]
fn strong_etag(etag: Option<&str>) -> Option<&str> {
    etag.filter(|value| !value.trim_start().starts_with("W/"))
}

/// What the resume probe learned about the object.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Default)]
struct SpanProbe {
    total: Option<u64>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// The full object metadata a span can use.
/// An open span requires the HEAD, a closed span treats it as best effort,
/// so a server without HEAD still serves an explicit `START-END`.
#[cfg(not(target_arch = "wasm32"))]
async fn probe_span(
    client: &NexusClient,
    url: &str,
    start: u64,
    end: Option<u64>,
) -> Result<SpanProbe, Error> {
    if end.is_some() {
        return Ok(match client.head_transfer(url).await {
            Ok(info) => SpanProbe {
                total: info.size,
                etag: info.etag,
                last_modified: info.last_modified,
            },
            Err(_) => SpanProbe::default(),
        });
    }
    let info = client.head_transfer(url).await?;
    let Some(size) = info.size else {
        return Err(Error::Misuse(format!(
            "the server did not report a size for {url}: the open-ended range needs a Content-Length; use START-END instead"
        )));
    };
    if start >= size {
        return Err(Error::Misuse(format!(
            "range starts at {start}, past the end of {size} bytes"
        )));
    }
    Ok(SpanProbe {
        total: Some(size),
        etag: info.etag,
        last_modified: info.last_modified,
    })
}

/// Resolve `START-END`, `START-` or the curl-style suffix `-N` into concrete bounds and a probe.
#[cfg(not(target_arch = "wasm32"))]
async fn resolve_span(
    client: &NexusClient,
    url: &str,
    spec: &str,
) -> Result<(u64, Option<u64>, SpanProbe), Error> {
    if let Some(suffix) = spec.strip_prefix('-') {
        let n = suffix
            .parse::<u64>()
            .map_err(|_| Error::Misuse(format!("range suffix is not a number: {spec}")))?;
        if n == 0 {
            return Err(Error::Misuse(
                "range suffix must cover at least one byte".to_owned(),
            ));
        }
        let info = client.head_transfer(url).await?;
        let Some(size) = info.size else {
            return Err(Error::Misuse(format!(
                "the server did not report a size for {url}: the suffix range needs a Content-Length"
            )));
        };
        if size == 0 {
            return Err(Error::Misuse(format!(
                "the object is empty: {url} has no suffix to read"
            )));
        }
        let start = size.saturating_sub(n);
        return Ok((
            start,
            None,
            SpanProbe {
                total: Some(size),
                etag: info.etag,
                last_modified: info.last_modified,
            },
        ));
    }
    let (start, end) = parse_range(spec)?;
    let probe = probe_span(client, url, start, end).await?;
    Ok((start, end, probe))
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
