use std::path::PathBuf;

use napi::bindgen_prelude::{Either, Uint8Array};
use napi::Result;
use napi_derive::napi;
use nexus_raw_core::model::digest;
use nexus_raw_core::{Error, GetOutcome, Nxr, ShaSource};
use tokio::sync::mpsc;

use crate::error::js_error;
use crate::mapping;
use crate::opts::{split_common, NxrCommonOpts, NxrGetOpts, NxrPutOpts};
use crate::pump::{finish_pump, finish_pump_err, spawn_pump};
use crate::result::{NxrGetResult, NxrHeadResult, NxrPutResult};

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
        o.ca_bundle,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let out = o.out.map(PathBuf::from);
    let pump = spawn_pump(rx, on_event);
    if let Some(spec) = o.range {
        if o.cont.unwrap_or(false) {
            drop(nxr);
            return Err(finish_pump_err(
                pump,
                Error::Misuse("range and cont exclude each other".into()),
            )
            .await);
        }
        let range = match nxr.get_range(&url, &spec).await {
            Ok(range) => range,
            Err(e) => return Err(finish_pump_err(pump, e).await),
        };
        if let Some(path) = out {
            if let Err(e) = nexus_raw_core::primitive::write_bytes_atomically(&path, &range.bytes) {
                drop(nxr);
                return Err(finish_pump_err(pump, e).await);
            }
        } else {
            let written = {
                use std::io::Write as _;
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(&range.bytes).and_then(|()| stdout.flush())
            };
            if let Err(e) = written {
                drop(nxr);
                return Err(finish_pump_err(
                    pump,
                    Error::Io {
                        path: "-".to_owned(),
                        detail: e.to_string(),
                    },
                )
                .await);
            }
        }
        let size = range.bytes.len();
        drop(nxr);
        finish_pump(pump).await?;
        return Ok(NxrGetResult {
            size: size as f64,
            sha256: None,
            resumed_from: 0.0,
        });
    }
    // A resume needs the sibling part file of a named output: without `out`
    // there is nothing to resume and the flag would be silently dead.
    if o.cont.unwrap_or(false) && out.is_none() {
        drop(nxr);
        return Err(finish_pump_err(
            pump,
            Error::Misuse(
                "continue resumes a sibling part file: pass out, or drop continue".into(),
            ),
        )
        .await);
    }
    let outcome = nxr.get(&url, out, o.cont.unwrap_or(false)).await;
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
/// A `Uint8Array` (including a Node `Buffer`) is the exact bytes, staged through a temporary file.
#[napi]
pub async fn put(
    url: String,
    source: Either<String, Uint8Array>,
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
        o.ca_bundle,
        o.on_event,
    );
    let cfg = mapping::build_config(&url, &common).map_err(js_error)?;
    let (tx, rx) = mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(js_error)?;
    let pump = spawn_pump(rx, on_event);
    // A byte body is staged through a temp file that lives until the PUT ends.
    // The file carries the artifact's own name, so the progress events name the
    // upload instead of a random temp file.
    let staged = match &source {
        Either::A(_) => None,
        Either::B(body) => {
            let dir = tempfile::Builder::new()
                .prefix("nxr-put-")
                .tempdir()
                .map_err(napi::Error::from)?;
            let leaf = url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("body.bin");
            let name = if nexus_raw_core::ArtifactName::parse(leaf).is_ok() {
                leaf
            } else {
                "body.bin"
            };
            let path = dir.path().join(name);
            std::fs::write(&path, body.as_ref()).map_err(napi::Error::from)?;
            Some((dir, path))
        }
    };
    let path = match (&source, &staged) {
        (Either::A(path), _) => PathBuf::from(path),
        (Either::B(_), Some((_, path))) => path.clone(),
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
        o.ca_bundle,
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
        // The CLI's classes: a missing path, a directory or a non-http scheme is
        // a command-line mistake (exit 2), anything else around the file is I/O
        // (exit 1).
        if let Some((scheme, _)) = target.split_once("://") {
            return Err(js_error(Error::Misuse(format!(
                "sha takes an http(s) URL or a local path, got a {scheme} URL"
            ))));
        }
        let path = PathBuf::from(&target);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => {
                return Err(js_error(Error::Misuse(format!(
                    "not a file: {}",
                    path.display()
                ))))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(js_error(Error::Misuse(format!(
                    "no such local file or http(s) URL: {target}"
                ))))
            }
            Err(e) => return Err(js_error(Error::io(&path, e))),
        }
        return digest::sha256_file(&path)
            .map(|d| d.as_str().to_owned())
            .map_err(|e| {
                js_error(Error::Io {
                    path: path.display().to_string(),
                    detail: e.to_string(),
                })
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
        o.ca_bundle,
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
