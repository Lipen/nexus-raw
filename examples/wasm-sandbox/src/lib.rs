//! The wasm face of the sandbox: `nxr_ls` and `nxr_put`.
//!
//! The browser owns TLS, timeouts and CORS policy, so the page always talks to a
//! same-origin `/nexus/` mount backed by a proxy (`serve.mjs --upstream`) or by
//! the built-in fake.
//! The read surface of [`Nxr`] needs no local filesystem and no spawned tasks,
//! which is exactly the slice that compiles for wasm32-unknown-unknown.
//! The in-memory PUT (`Nxr::put_bytes`) is the same kind of slice: bytes in, two
//! requests out, no file and no task.

use std::time::Duration;

use nexus_raw_core::config::Config;
use nexus_raw_core::layout::ls::EntryKind;
use nexus_raw_core::nxr::Nxr;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// A page is a politer client than a batch job: fewer workers and attempts than the CLI defaults.
/// Three attempts keep the retry path honest: two backoff sleeps survive any single flake.
const WORKERS: usize = 4;
const RETRY_ATTEMPTS: u32 = 3;

/// One listed child, as the page renders it.
#[derive(Serialize)]
struct Row {
    name: String,
    dir: bool,
}

/// The message plus the protocol hint, newline-joined.
fn complain(e: nexus_raw_core::Error) -> String {
    match e.hint() {
        Some(hint) => format!("{e}\nhint: {hint}"),
        None => e.to_string(),
    }
}

/// List the immediate children of a raw directory URL: folders first, then files, `.sha256` markers hidden.
///
/// `auth` is the raw `Authorization` header value (`Basic ...`), passed through to the proxy when present.
/// Returns a JSON array of `{name, dir}` rows. The error is a plain message string.
///
/// # Errors
///
/// Rejects with the transport, auth or enumerate message when the client cannot be built or the search API refuses.
#[wasm_bindgen]
pub async fn nxr_ls(dir_url: String, auth: Option<String>) -> Result<String, String> {
    let cfg = Config {
        base: dir_url,
        tls_insecure: false,
        workers: WORKERS,
        retry_attempts: RETRY_ATTEMPTS,
        connect_timeout: Duration::from_secs(15),
        stall_timeout: Duration::from_secs(30),
        auth,
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(complain)?;
    let entries = nxr.ls_entries().await.map_err(complain)?;
    let rows: Vec<Row> = entries
        .into_iter()
        .map(|e| Row {
            name: e.name,
            dir: e.kind == EntryKind::Dir,
        })
        .collect();
    serde_json::to_string(&rows).map_err(|e| e.to_string())
}

/// PUT bytes with their sha-sibling marker: the browser upload call.
///
/// `file_url` is the object URL, `bytes` the whole file, and `auth` the raw
/// `Authorization` header value when the proxy needs it.
/// Returns a JSON object `{"bytes": N, "digest": "..."}`.
///
/// # Errors
///
/// Rejects with the transport, auth or HTTP message when the upload or the
/// marker fails.
#[wasm_bindgen]
pub async fn nxr_put(
    file_url: String,
    bytes: Vec<u8>,
    auth: Option<String>,
) -> Result<String, String> {
    let cfg = Config {
        base: file_url.clone(),
        tls_insecure: false,
        workers: WORKERS,
        retry_attempts: RETRY_ATTEMPTS,
        connect_timeout: Duration::from_secs(15),
        stall_timeout: Duration::from_secs(30),
        auth,
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let nxr = Nxr::new(cfg, tx).map_err(complain)?;
    let (bytes, digest) = nxr
        .put_bytes(&file_url, &bytes, true)
        .await
        .map_err(complain)?;
    serde_json::to_string(&serde_json::json!({
        "bytes": bytes,
        "digest": digest.map(|digest| digest.to_string()),
    }))
    .map_err(|e| e.to_string())
}
