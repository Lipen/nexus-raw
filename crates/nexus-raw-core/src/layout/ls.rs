//! Best-effort listings through the Nexus search API (spec §5.3, §10).
//!
//! The endpoint is `/service/rest/v1/search/assets`; it exists on common
//! Nexus 3 releases but is not guaranteed. Every refusal is an
//! [`Error::Enumerate`] with a hint, never a silent empty list.

use crate::error::Error;
use crate::model::name::validate_version;
use crate::model::name::ArtifactName;
use crate::transport::client::NexusClient;

/// Versions under a repository/group base: the path segment right after the
/// group prefix of every asset, collected and sorted.
///
/// The base is a *prefix* here: `<…>/repository/<repo>/<group…>/` with any
/// number of group segments, including none.
pub async fn search_versions(client: &NexusClient, base: &str) -> Result<Vec<String>, Error> {
    let (repo, group) = split_prefix(base)?;
    let paths = paginate(client, base, &repo, &group).await?;
    let mut versions = std::collections::BTreeSet::new();
    for path in paths {
        let rel = drop_segments(&path, group.len());
        if let Some(seg) = rel.split('/').next() {
            if !seg.is_empty() && validate_version(seg).is_ok() {
                versions.insert(seg.to_owned());
            }
        }
    }
    Ok(versions.into_iter().collect())
}

/// Artifact names under a version directory URL: every asset path that starts
/// with `<group>/<version>/`, mapped to the relative name.
pub async fn search_assets(
    client: &NexusClient,
    dir_url: &str,
) -> Result<Vec<ArtifactName>, Error> {
    let parts = split_base(dir_url)?;
    let paths = paginate(client, dir_url, &parts.repo, &parts.group).await?;
    let mut names = Vec::new();
    for path in paths {
        let rel = drop_segments(&path, parts.group.len());
        let Some(rel) = rel.strip_prefix(&format!("{}/", parts.version)) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        names.push(ArtifactName::parse(rel)?);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

struct BaseParts {
    repo: String,
    group: Vec<String>,
    version: String,
}

/// `<…>/repository/<repo>/<group…>/` as a prefix: the group is everything
/// after the repo, and there is no version segment requirement.
fn split_prefix(base: &str) -> Result<(String, Vec<String>), Error> {
    let err = || {
        Error::misuse(format!(
            "URL {base:?} must point inside /repository/<name>/<group…>/"
        ))
    };
    let url = reqwest::Url::parse(base).map_err(|_| err())?;
    let segs: Vec<String> = url
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();
    if segs.len() < 2 || segs[0] != "repository" {
        return Err(err());
    }
    Ok((segs[1].clone(), segs[2..].to_vec()))
}

/// `<scheme>://host/repository/<repo>/<group…>/<version>/` → parts.
/// The version is the last segment; the group is what is between the repo and it.
fn split_base(base: &str) -> Result<BaseParts, Error> {
    let err = || {
        Error::misuse(format!(
            "URL {base:?} must point inside /repository/<name>/<group…>/<version>/"
        ))
    };
    let url = reqwest::Url::parse(base).map_err(|_| err())?;
    let segs: Vec<String> = url
        .path()
        .split('/')
        .filter(|s| !s.is_empty())
        .map(percent_decode)
        .collect();
    if segs.len() < 3 || segs[0] != "repository" {
        return Err(err());
    }
    Ok(BaseParts {
        repo: segs[1].clone(),
        group: segs[2..segs.len() - 1].to_vec(),
        version: segs[segs.len() - 1].clone(),
    })
}

/// Drop the first `n` path segments.
fn drop_segments(path: &str, n: usize) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .skip(n)
        .collect::<Vec<_>>()
        .join("/")
}

/// Walk the search pages, collecting asset paths.
async fn paginate(
    client: &NexusClient,
    base: &str,
    repo: &str,
    group: &[String],
) -> Result<Vec<String>, Error> {
    let url = reqwest::Url::parse(base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
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
    let mut paths = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page = endpoint.clone();
        if let Some(token) = &next {
            page.query_pairs_mut()
                .append_pair("continuationToken", token);
        }
        let page_url = page.to_string();
        let bytes = client.get_small(&page_url).await?;
        let Some(bytes) = bytes else {
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
                if let Some(p) = item.get("path").and_then(|p| p.as_str()) {
                    paths.push(p.to_owned());
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
    Ok(paths)
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
    fn base_split_repo_group_version() {
        let p = split_base("http://h/repository/raw/lib/app/1.4.0/").unwrap();
        assert_eq!(p.repo, "raw");
        assert_eq!(p.group, ["lib", "app"]);
        assert_eq!(p.version, "1.4.0");
        let p = split_base("http://h/repository/raw/1.4.0/").unwrap();
        assert_eq!(p.group, Vec::<String>::new());
        assert_eq!(p.version, "1.4.0");
        assert!(split_base("http://h/other/raw/x/").is_err());
    }

    #[test]
    fn segment_dropping() {
        assert_eq!(drop_segments("lib/app/1.4.0/a.zip", 2), "1.4.0/a.zip");
        assert_eq!(drop_segments("1.4.0/a.zip", 0), "1.4.0/a.zip");
        assert_eq!(drop_segments("", 3), "");
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%2"), "bad%2");
    }
}
