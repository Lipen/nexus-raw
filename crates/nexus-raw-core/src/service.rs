//! Server metadata through the Sonatype service REST API (the management surface).
//!
//! These endpoints are not part of the storage protocol: the storage contract lives in the
//! crate docs, and a server that lacks the service API still serves every storage invariant.
//! `nxr` reads the repositories document for human convenience only (the panel use case),
//! with the same credentials and the same transport as everything else.

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::transport::client::NexusClient;

/// One repository of a Nexus server, as the service REST API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// Repository name.
    pub name: String,
    /// Repository format (`raw`, `maven2`, `docker`, ...), spelled as the server spells it.
    pub format: String,
    /// Repository kind (`hosted`, `proxy`, `group`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Repository URL.
    pub url: String,
}

/// The server root of a directory URL: scheme, host and port, without the path.
///
/// `https://nexus.example.com/repository/raw-main/1.4.0/` roots to `https://nexus.example.com`,
/// so a repository URL works wherever the server root is expected.
fn server_root(base: &str) -> Result<String, Error> {
    let url = reqwest::Url::parse(base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
    let host = url.host_str().unwrap_or_default();
    let mut root = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        root.push_str(&format!(":{port}"));
    }
    Ok(root)
}

/// List the repositories of the server `base` belongs to:
/// `GET <root>/service/rest/v1/repositories`, the same call the web console makes.
pub(crate) async fn repositories(client: &NexusClient, base: &str) -> Result<Vec<RepoInfo>, Error> {
    let root = server_root(base)?;
    let url = format!("{root}/service/rest/v1/repositories");
    let Some(bytes) = client.get_small(&url).await? else {
        return Err(Error::ServiceMissing {
            url: url.clone(),
            root,
        });
    };
    serde_json::from_slice(&bytes).map_err(|e| Error::Mismatch {
        name: url.clone(),
        detail: format!("the service response is not a repository list: {e}"),
    })
}

/// Server liveness and write access: `/service/rest/v1/status` and `/status/writable`.
///
/// An empty 200 body is the norm on Nexus 3.79 and is not an error;
/// a JSON body carrying `version` surfaces it, anything else is ignored.
/// A server without these endpoints (the mock, an old Nexus) reads `alive: false`:
/// the verdict is about the endpoint, not about the TCP reachability - `doctor` owns that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusReport {
    /// The server root the report was resolved against.
    pub root: String,
    /// The status endpoint answered 200.
    pub alive: bool,
    /// The writable endpoint answered 200 (false: read-only or refused).
    pub writable: bool,
    /// The server version, when the status body carried one.
    pub version: Option<String>,
}

/// Liveness and writability of the server `base` belongs to.
pub(crate) async fn status(client: &NexusClient, base: &str) -> Result<StatusReport, Error> {
    let root = server_root(base)?;
    let alive = probe_ok(client, &format!("{root}/service/rest/v1/status")).await?;
    let writable =
        alive && probe_ok(client, &format!("{root}/service/rest/v1/status/writable")).await?;
    let version = if alive {
        client
            .get_small(&format!("{root}/service/rest/v1/status"))
            .await?
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_owned))
    } else {
        None
    };
    Ok(StatusReport {
        root,
        alive,
        writable,
        version,
    })
}

/// True when the URL answers 200; any other verdict (404, 403, transport) is `false`.
async fn probe_ok(client: &NexusClient, url: &str) -> Result<bool, Error> {
    Ok(client.get_small(url).await?.is_some())
}

/// One repository entry with its optional full settings (the admin-only detail).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoReport {
    /// The trimmed collection entry every allowed user sees.
    #[serde(flatten)]
    pub info: RepoInfo,
    /// Full settings, only for an nx-admin caller with `--detail`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

/// Resolve the repository a URL belongs to and, optionally, its admin-only settings.
///
/// The resolution is a longest-prefix match of `url` against the collection entries.
pub(crate) async fn repo(
    client: &NexusClient,
    base: &str,
    detail: bool,
) -> Result<RepoReport, Error> {
    let root = server_root(base)?;
    let url = format!("{root}/service/rest/v1/repositories");
    let Some(bytes) = client.get_small(&url).await? else {
        return Err(Error::ServiceMissing {
            url: url.clone(),
            root,
        });
    };
    let list: Vec<RepoInfo> = serde_json::from_slice(&bytes).map_err(|e| Error::Mismatch {
        name: url.clone(),
        detail: format!("the service response is not a repository list: {e}"),
    })?;
    let needle = base.trim_end_matches('/');
    let mut best: Option<&RepoInfo> = None;
    for entry in &list {
        let candidate = entry.url.trim_end_matches('/');
        let matches = needle == candidate || needle.starts_with(&format!("{candidate}/"));
        if matches && best.is_none_or(|b| candidate.len() > b.url.trim_end_matches('/').len()) {
            best = Some(entry);
        }
    }
    let Some(info) = best.cloned() else {
        return Err(Error::Missing {
            names: vec![base.to_owned()],
        });
    };
    let mut report = RepoReport { info, detail: None };
    if detail {
        let detail_url = format!(
            "{root}/service/rest/v1/repositories/{}/{}/{}",
            report.info.format, report.info.kind, report.info.name
        );
        report.detail = match client.get_small(&detail_url).await? {
            Some(bytes) => Some(serde_json::from_slice(&bytes).map_err(|e| Error::Mismatch {
                name: detail_url.clone(),
                detail: format!("the repository detail is not JSON: {e}"),
            })?),
            None => None,
        };
    }
    Ok(report)
}

/// One asset of a repository, as the search API reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetEntry {
    /// Asset path inside the repository, without the leading slash.
    pub path: String,
    /// The absolute download URL.
    pub url: String,
    /// Asset size in bytes, when the server reports it.
    pub size: Option<u64>,
    /// The sha256 digest, when the server reports it.
    pub sha256: Option<String>,
    /// The last modification timestamp, when the server reports it.
    pub last_modified: Option<String>,
}

/// The tail event of an asset listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssetsSummary {
    /// How many assets the listing produced.
    pub assets: usize,
    /// How many search pages were fetched.
    pub pages: usize,
}

/// List every asset of the repository `base` belongs to, through the search API.
///
/// `q` is passed through to the server; `prefix` filters whole segments client-side.
pub(crate) async fn assets(
    client: &NexusClient,
    base: &str,
    q: Option<&str>,
    prefix: &[String],
) -> Result<(Vec<AssetEntry>, AssetsSummary), Error> {
    let url = reqwest::Url::parse(base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
    let mut origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    // The repository name is the segment after /repository/ in the directory URL.
    let path = url.path().trim_end_matches('/');
    let repo = path
        .strip_prefix("/repository/")
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(|| {
            Error::misuse(format!(
                "{base}: assets need a repository URL (/repository/<name>/), not a server root"
            ))
        })?
        .to_owned();
    let mut endpoint = reqwest::Url::parse(&format!("{origin}/service/rest/v1/search/assets"))
        .map_err(|e| Error::misuse(format!("search endpoint: {e}")))?;
    endpoint.query_pairs_mut().append_pair("repository", &repo);
    if let Some(q) = q {
        endpoint.query_pairs_mut().append_pair("q", q);
    }
    let mut entries = Vec::new();
    let mut pages = 0usize;
    let mut next: Option<String> = None;
    // A hostile or broken endpoint can emit continuation tokens forever.
    // The page cap turns an endless scroll into an Enumerate refusal.
    const MAX_SEARCH_PAGES: usize = 1000;
    loop {
        pages += 1;
        if pages > MAX_SEARCH_PAGES {
            return Err(Error::Enumerate {
                url: endpoint.to_string(),
                reason: format!("search pagination exceeded {MAX_SEARCH_PAGES} pages"),
            });
        }
        let mut page = endpoint.clone();
        if let Some(token) = &next {
            page.query_pairs_mut()
                .append_pair("continuationToken", token);
        }
        let page_url = page.to_string();
        let Some(bytes) = client.get_small(&page_url).await? else {
            return Err(Error::Enumerate {
                url: page_url,
                reason: "the search endpoint answered 404".into(),
            });
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Enumerate {
                url: page_url.clone(),
                reason: format!("search response is not JSON: {e}"),
            })?;
        if let Some(items) = value.get("items").and_then(|i| i.as_array()) {
            for item in items {
                let Some(p) = item.get("path").and_then(|p| p.as_str()) else {
                    continue;
                };
                let path = p.trim_start_matches('/');
                if !prefix.is_empty()
                    && !prefix.iter().any(|pre| {
                        path == pre.trim_end_matches('/')
                            || path.starts_with(&format!("{}/", pre.trim_end_matches('/')))
                    })
                {
                    continue;
                }
                let download = item
                    .get("downloadUrl")
                    .and_then(|d| d.as_str())
                    .map(str::to_owned);
                let url = match download {
                    Some(u) => u,
                    None => format!("{origin}/repository/{repo}/{path}"),
                };
                entries.push(AssetEntry {
                    path: path.to_owned(),
                    url,
                    size: item.get("size").and_then(|s| s.as_u64()),
                    sha256: item
                        .get("checksum")
                        .and_then(|c| c.get("sha256"))
                        .and_then(|s| s.as_str())
                        .map(str::to_owned),
                    last_modified: item
                        .get("lastModified")
                        .and_then(|s| s.as_str())
                        .map(str::to_owned),
                });
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
    let count = entries.len();
    Ok((
        entries,
        AssetsSummary {
            assets: count,
            pages,
        },
    ))
}

/// The EULA gate state of a CE server (3.79+): the onboarding verdict and its disclaimer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EulaStatus {
    /// Whether the EULA was accepted.
    pub accepted: bool,
    /// The disclaimer text: echo it back verbatim to accept.
    pub disclaimer: String,
}

/// The outcome of an ensure-accepted EULA pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EulaOutcome {
    /// The gate state after the call.
    pub accepted: bool,
    /// What this call did: `none` (already accepted or no gate), `accepted`.
    pub action: String,
}

/// Read the EULA gate: `GET <root>/service/rest/v1/system/eula`.
///
/// A 404 means the server has no gate (older CE, PRO, or a non-Sonatype server): `None`.
/// A 403 means the gate exists but this user may not even read it: `None` with the refusal noted in the hint-free output; `--accept` will fail the same way.
pub(crate) async fn eula(client: &NexusClient, base: &str) -> Result<Option<EulaStatus>, Error> {
    let root = server_root(base)?;
    let url = format!("{root}/service/rest/v1/system/eula");
    match client.get_small(&url).await {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::Mismatch {
                name: url.clone(),
                detail: format!("the eula response is not JSON: {e}"),
            }),
        Ok(None) => Ok(None),
        Err(Error::Auth { .. }) => {
            // The gate exists (3.79+ answers it for admins) but this user is not trusted to read it.
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Ensure the EULA gate is open: accept it when the server presents the disclaimer.
///
/// The disclaimer is echoed verbatim: the server refuses a rewritten one.
pub(crate) async fn eula_accept(client: &NexusClient, base: &str) -> Result<EulaOutcome, Error> {
    let Some(status) = eula(client, base).await? else {
        return Ok(EulaOutcome {
            accepted: false,
            action: "none".to_owned(),
        });
    };
    if status.accepted {
        return Ok(EulaOutcome {
            accepted: true,
            action: "none".to_owned(),
        });
    }
    let root = server_root(base)?;
    let url = format!("{root}/service/rest/v1/system/eula");
    let body = serde_json::json!({ "disclaimer": status.disclaimer, "accepted": true });
    client
        .post_small(&url, body.to_string().into_bytes())
        .await?;
    Ok(EulaOutcome {
        accepted: true,
        action: "accepted".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_root_cuts_the_repository_path() {
        assert_eq!(
            server_root("https://nexus.example.com/repository/raw-main/1.4.0/").unwrap(),
            "https://nexus.example.com"
        );
        assert_eq!(
            server_root("http://127.0.0.1:8081/repository/raw-main/").unwrap(),
            "http://127.0.0.1:8081"
        );
        assert_eq!(
            server_root("https://nexus.example.com/").unwrap(),
            "https://nexus.example.com"
        );
    }
}
