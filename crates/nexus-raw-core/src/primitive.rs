//! L0 primitives: curl-grade single-object operations (spec §5.1).
//!
//! Retry, stall detection, TLS and auth come from the transport.
//! Primitives never classify, never diff and never refuse to overwrite:
//! that is the transfer layer's job.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt;

use crate::error::Error;
use crate::events::Dir;
use crate::model::digest::{self, Digest};
use crate::model::name::ArtifactName;
use crate::model::sibling;
pub use crate::transport::client::HeadInfo;
use crate::transport::client::NexusClient;

/// The `get` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetOutcome {
    /// Final size in bytes (stdout mode: bytes written).
    pub size: u64,
    /// The digest, when a file was produced (stdout streaming does not hash).
    pub digest: Option<Digest>,
    /// The offset the transfer resumed from (0 for a fresh download).
    pub resumed_from: u64,
}

/// GET a URL.
///
/// With `out`: stream into `<out>.part` (resuming from it when `cont`), verify nothing, then rename to `out`.
/// Without `out`: stream to stdout in a single body attempt, because a retry after the body started would duplicate bytes.
///
/// # Errors
///
/// Returns transport, auth or HTTP errors from the GET, [`Error::Io`] when the part file cannot be written or renamed, and [`Error::Misuse`] when stdout cannot be written.
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
                .map_or(0, |m| m.len())
        } else {
            0
        };
        let name = out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (size, digest) = match client
            .download_resumable((&name, Dir::Down), url, &part, cont, None)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tokio::fs::remove_file(&part).await.ok();
                return Err(e);
            }
        };
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
        let total = resp.content_length();
        // The contract: Started precedes the body it announces, even in stdout mode where nothing else is written.
        client.progress().started("", Dir::Down, total).await;
        let mut body = resp.bytes_stream();
        let mut stdout = tokio::io::stdout();
        let mut size: u64 = 0;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| Error::transport(url, e))?;
            stdout
                .write_all(&chunk)
                .await
                .map_err(|e| Error::misuse(format!("stdout: {e}")))?;
            size += chunk.len() as u64;
        }
        stdout
            .flush()
            .await
            .map_err(|e| Error::misuse(format!("stdout: {e}")))?;
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
    client.upload_file((&name, Dir::Up), url, src, size).await?;
    if !sha {
        return Ok((size, None));
    }
    let d = digest::sha256_file(src).map_err(|e| Error::io(src, e))?;
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
pub enum ShaSource {
    File(PathBuf),
    Url(String),
}

/// Stream a source through sha256 (spec §5.1 `sha <file|url>`).
///
/// # Errors
///
/// Returns [`Error::Io`] when a file source cannot be read and transport, auth or HTTP errors for a URL source.
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

/// `<out>.part` next to the output file.
fn part_of(out: &Path) -> PathBuf {
    let mut s = out.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

/// The final path segment of a URL, percent-decoded: the marker's name field.
fn last_segment(url: &str) -> String {
    let raw = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.path().rsplit('/').next().map(str::to_owned))
        .unwrap_or_default();
    percent_decode(&raw)
}

fn percent_decode(seg: &str) -> String {
    let bytes = seg.as_bytes();
    let hex = |b: u8| -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
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
}
