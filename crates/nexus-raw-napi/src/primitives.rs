use std::path::PathBuf;

use napi::bindgen_prelude::{Buffer, Either};
use napi::Result;
use napi_derive::napi;
use nexus_raw_core::model::digest;
use nexus_raw_core::{GetOutcome, Nxr, ShaSource};
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
