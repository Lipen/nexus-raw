//! Best-effort listings through the Nexus search API (spec §5.3, §10).
//!
//! The endpoint is `/service/rest/v1/search/assets`.
//! A directory listing asks it with `q=<percent-encoded directory path>/`, which cuts the
//! pages down to that directory.
//! The server's `q` match is loose (a path substring), so the client-side prefix filter stays
//! the correctness mechanism, and `q` only saves requests.
//! A repository-wide walk (`audit`, the delta's search path) asks without `q`: there the page
//! set must be the repository, and a filter mismatch would be a silent hole.
//! It exists on common Nexus 3 releases but is not guaranteed.
//! Every refusal is an error with a hint, never a silent empty list: a missing endpoint is an [`Error::Enumerate`], a refused repository is an [`Error::SearchRepoMissing`].

use crate::error::Error;
use crate::model::digest::Digest;
use crate::model::name::percent_decode;
use crate::model::name::ArtifactName;
use crate::transport::client::NexusClient;

/// One immediate child of a raw directory: a folder or a file.
///
/// A raw repository is an arbitrary tree: there are no version or object
/// concepts here, just names with an optional subtree below them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The child segment name.
    pub name: String,
    /// Whether the child has a subtree below it.
    pub kind: EntryKind,
}

/// Whether an entry is a folder or a leaf file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The child has more path segments below it.
    Dir,
    /// The child is a leaf object.
    File,
}

/// The immediate children of a raw directory URL: folders first, then files, each sorted.
///
/// Works at any depth, because the raw tree has no fixed shape: the search is scoped to the
/// repository and asks with `q=<directory>/` to cut the pages, and the prefix filter is
/// applied client-side, so the possible superset `q` serves becomes the exact answer.
/// The `group` search parameter is deliberately not used: on a real Nexus release it matched
/// Maven coordinates rather than raw path prefixes, which made nested listings lie.
/// Leaf `.sha256` siblings are hidden: they are derived data the protocol generates.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `dir_url` is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
pub async fn search_entries(client: &NexusClient, dir_url: &str) -> Result<Vec<Entry>, Error> {
    Ok(search_entries_counted(client, dir_url).await?.0)
}

/// The same listing, plus the count of `.sha256` markers the enumerator hid:
/// a silent undercount becomes a visible one (a user-facing report asked for
/// exactly this after the markers vanished from a version listing).
///
/// # Errors
///
/// The same as [`search_entries`].
pub async fn search_entries_counted(
    client: &NexusClient,
    dir_url: &str,
) -> Result<(Vec<Entry>, usize), Error> {
    let (repo, prefix) = split_prefix(dir_url)?;
    let paths = paginate(client, dir_url, &repo, scope_path(&prefix).as_deref()).await?;
    let mut dirs = std::collections::BTreeSet::new();
    let mut files = std::collections::BTreeSet::new();
    let mut hidden = 0usize;
    for path in paths {
        let segs: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect();
        if segs.len() <= prefix.len() || segs[..prefix.len()] != prefix[..] {
            continue;
        }
        let rel = &segs[prefix.len()..];
        let first = &rel[0];
        if first.is_empty() {
            continue;
        }
        if rel.len() == 1 {
            // A leaf marker is derived data, not a tree member: hidden, like `--assets` skips it.
            if first.ends_with(".sha256") {
                hidden += 1;
                continue;
            }
            files.insert(first.clone());
        } else {
            dirs.insert(first.clone());
        }
    }
    let entries = dirs
        .into_iter()
        .map(|name| Entry {
            name,
            kind: EntryKind::Dir,
        })
        .chain(files.into_iter().map(|name| Entry {
            name,
            kind: EntryKind::File,
        }))
        .collect();
    Ok((entries, hidden))
}

/// Artifact names under a version directory URL: every asset path that starts with `<group>/<version>/`, mapped to the relative name.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `dir_url` is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
pub async fn search_assets(
    client: &NexusClient,
    dir_url: &str,
) -> Result<Vec<ArtifactName>, Error> {
    let parts = split_base(dir_url)?;
    let scope = directory_scope(&parts.group, &parts.version);
    let paths = paginate(client, dir_url, &parts.repo, scope.as_deref()).await?;
    let mut names = Vec::new();
    for path in paths {
        let rel = drop_segments(&path, parts.group.len());
        let Some(rel) = rel.strip_prefix(&format!("{}/", parts.version)) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        // The search sees markers too, and a marker name carries the reserved
        // suffix: it is not an artifact, so skip it instead of failing the
        // whole listing on it.
        let Ok(name) = ArtifactName::parse(rel) else {
            continue;
        };
        names.push(name);
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// Every artifact name under a repository prefix, with the search-reported
/// facts per name.
///
/// The prefix is any directory inside `/repository/<repo>/`, including the
/// repository root.
/// The walk is repository-wide even when a prefix is given: `q` is a loose
/// server substring match, and a clone is a backup whose page set must be
/// complete, so a `q` miss would be a silent hole the client-side prefix
/// filter could not recover.
/// The whole-segment prefix filter is applied client-side and is the exact
/// answer.
/// Leaf `.sha256` siblings are hidden, and names that fail the artifact
/// grammar are skipped rather than failing the whole walk.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `base` does not point inside a repository,
/// [`Error::SearchRepoMissing`] on a search 400 and [`Error::Enumerate`] on a
/// search 404 or an unparseable page.
pub async fn search_repo_assets(
    client: &NexusClient,
    base: &str,
) -> Result<Vec<(ArtifactName, Option<Digest>, Option<u64>)>, Error> {
    let (repo, prefix) = split_prefix(base)?;
    let assets = walk(client, base, &repo, None).await?;
    let mut out = Vec::new();
    for asset in assets {
        let rel = drop_segments(&asset.path, prefix.len());
        if rel.is_empty() || rel.ends_with(".sha256") {
            continue;
        }
        let Ok(name) = ArtifactName::parse(&rel) else {
            continue;
        };
        out.push((name, asset.sha256, asset.size));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

/// The same walk as [`search_assets`], carrying the search-reported facts per name:
/// the digest and size the server's index holds, so a comparison runs without
/// a single per-name HEAD or sibling GET.
///
/// A name the search reports without a digest still lists: the comparison sees
/// the missing fact, never a fake equality.
///
/// The walk stays repository-wide on purpose: this is the delta's remote side, where a
/// narrowed page set would report existing objects as absent, so `q` is not sent.
///
/// # Errors
///
/// Returns [`Error::Misuse`] when `dir_url` is not a directory URL and [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
// The fact type lives in the native-only sync family.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn search_assets_facts(
    client: &NexusClient,
    dir_url: &str,
) -> Result<Vec<(ArtifactName, crate::sync::delta::Side)>, Error> {
    let parts = split_base(dir_url)?;
    let assets = walk(client, dir_url, &parts.repo, None).await?;
    let version_prefix = format!("{}/", parts.version);
    let mut out = Vec::new();
    for asset in assets {
        let rel = drop_segments(&asset.path, parts.group.len());
        let Some(rel) = rel.strip_prefix(&version_prefix) else {
            continue;
        };
        if rel.is_empty() {
            continue;
        }
        // The search sees markers too, and a marker name carries the reserved
        // suffix: it is not an artifact, so skip it instead of failing the
        // whole listing on it.
        let Ok(name) = ArtifactName::parse(rel) else {
            continue;
        };
        out.push((
            name,
            crate::sync::delta::Side {
                size: asset.size,
                digest: asset.sha256,
            },
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

/// The audit listing: every repository object as served, markers included,
/// with the search facts per object.
///
/// Unlike [`search_assets`] nothing is filtered or grammar-checked here: the
/// audit needs the raw truth of what the storage lists, and a marker name
/// (which the artifact grammar refuses by the reserved suffix) is exactly the
/// kind of object the audit must see.
///
/// The walk stays repository-wide on purpose: `q` is a filter, and a filter the audit
/// trusts is a hole in the audit.
///
/// # Errors
///
/// Returns [`Error::Enumerate`] when the search endpoint is unavailable or
/// unparseable and [`Error::SearchRepoMissing`] on a search 400.
pub(crate) async fn audit_tree(
    client: &NexusClient,
    dir_url: &str,
) -> Result<Vec<(String, Option<Digest>)>, Error> {
    let parts = split_base(dir_url)?;
    let assets = walk(client, dir_url, &parts.repo, None).await?;
    let version_prefix = format!("{}/", parts.version);
    let mut out: Vec<(String, Option<Digest>)> = assets
        .into_iter()
        .filter_map(|a| {
            let rel = drop_segments(&a.path, parts.group.len());
            let rel = rel.strip_prefix(&version_prefix)?;
            Some((rel.to_owned(), a.sha256))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    Ok(out)
}

struct BaseParts {
    repo: String,
    group: Vec<String>,
    version: String,
}

/// `<…>/repository/<repo>/<group…>/` as a prefix: the group is everything after the repo, and there is no version segment requirement.
pub(crate) fn split_prefix(base: &str) -> Result<(String, Vec<String>), Error> {
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
/// The version is the last segment.
/// The group is what is between the repo and it.
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

/// The `q` value a directory listing asks with: the directory path with a trailing slash.
///
/// The trailing slash is what keeps a name-continuing sibling (`flat` against `flat2`) out of
/// the cut. The server matches the value loosely (a case-sensitive path substring), so the
/// page set is a possible superset and the client-side prefix filter is what makes it exact.
/// The repository root has no scope: its walk is the repository, and `q` would only pretend.
fn scope_path(segments: &[String]) -> Option<String> {
    (!segments.is_empty()).then(|| format!("{}/", segments.join("/")))
}

/// The same scope for a `<group…>/<version>/` directory, which always has at least the version.
fn directory_scope(group: &[String], version: &str) -> Option<String> {
    let mut segments = group.to_vec();
    segments.push(version.to_owned());
    scope_path(&segments)
}

/// Walk the search pages, collecting asset paths.
///
/// `q` is the directory scope, or `None` for a repository-wide walk.
/// The `group` search parameter is deliberately never sent: on a real Nexus it
/// matches Maven coordinates, not raw path prefixes, which emptied deep
/// listings.
/// `q` cuts the pages, and the caller's prefix filter decides the answer.
async fn paginate(
    client: &NexusClient,
    base: &str,
    repo: &str,
    q: Option<&str>,
) -> Result<Vec<String>, Error> {
    Ok(walk(client, base, repo, q)
        .await?
        .into_iter()
        .map(|a| a.path)
        .collect())
}

/// One search-API asset, reduced to the facts a digest comparison needs.
#[derive(Debug, Clone)]
pub(crate) struct SearchAsset {
    /// Asset path inside the repository, without the leading slash (percent-encoded as served).
    pub path: String,
    /// The sha256 digest, when the server reports one.
    pub sha256: Option<Digest>,
    /// Asset size in bytes, when the server reports it.
    pub size: Option<u64>,
}

/// Walk the search pages, collecting per-asset facts: path, digest, size.
///
/// The same scoping rules as [`paginate`]: `q` is the directory scope or `None` for a
/// repository-wide walk, the `group` parameter is never sent, and the caller filters client-side.
/// The page cap, the 400 → [`Error::SearchRepoMissing`] mapping and the
/// 404 → [`Error::Enumerate`] mapping are shared with every listing helper.
async fn walk(
    client: &NexusClient,
    base: &str,
    repo: &str,
    q: Option<&str>,
) -> Result<Vec<SearchAsset>, Error> {
    let url = reqwest::Url::parse(base).map_err(|e| Error::misuse(format!("base URL: {e}")))?;
    let mut origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default());
    if let Some(port) = url.port() {
        origin.push_str(&format!(":{port}"));
    }
    let mut endpoint = reqwest::Url::parse(&format!("{origin}/service/rest/v1/search/assets"))
        .map_err(|e| Error::misuse(format!("search endpoint: {e}")))?;
    endpoint.query_pairs_mut().append_pair("repository", repo);
    if let Some(q) = q {
        // Percent-encoded by the URL builder, which is the form the server is measured to accept.
        endpoint.query_pairs_mut().append_pair("q", q);
    }
    let mut assets = Vec::new();
    let mut next: Option<String> = None;
    // A hostile or broken endpoint can emit continuation tokens forever.
    // The page cap turns an endless scroll into an Enumerate refusal.
    //
    // The bound is per walk, and a directory walk carries `q`, so the rows it covers are the
    // directory's, not the repository's.
    // Measured against the pinned Nexus 3.96.3
    // (stand/real-nexus/search-pages.sh): `/search/assets` serves 50 rows a page and ignores
    // `pageSize`, 6095 repository objects are 122 pages, and a 5000-name directory is 101.
    // 1000 pages is 50 000 rows: ten times that directory, eight times that repository, and
    // still a bound on a token stream that never ends.
    const MAX_SEARCH_PAGES: usize = 1000;
    let mut pages = 0usize;
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
        let bytes = match client.get_small(&page_url).await {
            Ok(bytes) => bytes,
            // A repository-scoped search for a repository the server does not serve is refused with 400: the repository is missing or is not raw.
            Err(Error::Http { status: 400, url }) => return Err(Error::SearchRepoMissing { url }),
            Err(e) => return Err(e),
        };
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
                let Some(p) = item.get("path").and_then(|p| p.as_str()) else {
                    continue;
                };
                assets.push(SearchAsset {
                    path: p.to_owned(),
                    sha256: item
                        .get("checksum")
                        .and_then(|c| c.get("sha256"))
                        .and_then(|s| s.as_str())
                        .and_then(|s| Digest::from_hex(s).ok()),
                    size: item.get("size").and_then(|s| s.as_u64()),
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
    Ok(assets)
}

/// The set of repo-relative asset paths the search index currently reports for the repository.
/// A visibility check reads this, not an enumeration: markers included, filtering is the caller's.
/// The walk is repository-wide: a name could be anywhere, so `q` is not sent.
/// # Errors
/// [`Error::Enumerate`] when the search endpoint is unavailable or unparseable.
pub(crate) async fn search_paths(
    client: &NexusClient,
    base: &str,
    repo: &str,
) -> Result<std::collections::HashSet<String>, Error> {
    Ok(paginate(client, base, repo, None)
        .await?
        .into_iter()
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An endpoint that never stops emitting continuation tokens hits the page cap and is refused, instead of scrolling forever.
    #[tokio::test]
    async fn pagination_cap_refuses_endless_tokens() {
        let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic).unwrap();
        let page = serde_json::json!({
            "items": [],
            "continuationToken": "more",
        });
        server.insert("service/rest/v1/search/assets", page.to_string().as_bytes());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cfg = crate::config::Config {
            base: server.base_url(),
            tls_insecure: false,
            ca_bundle: None,
            workers: 1,
            retry_attempts: 1,
            connect_timeout: std::time::Duration::from_secs(5),
            stall_timeout: std::time::Duration::from_secs(5),
            auth: None,
        };
        let client = NexusClient::new(&cfg, crate::events::Progress::new(tx)).unwrap();

        let err = paginate(&client, server.base_url().as_str(), "raw", None)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("pagination exceeded"),
            "the cap fires: {err}"
        );
    }

    /// The scope a directory listing asks with: the directory path and a trailing slash.
    /// The root has no scope, and the slash is what stops a name-continuing sibling.
    #[test]
    fn directory_scope_carries_the_trailing_slash() {
        let seg = |s: &str| s.to_owned();
        assert_eq!(scope_path(&[]), None, "the repository root is not a scope");
        assert_eq!(
            scope_path(&[seg("bench"), seg("flat")]),
            Some("bench/flat/".into())
        );
        assert_eq!(
            directory_scope(&[seg("lib"), seg("app")], "1.4.0"),
            Some("lib/app/1.4.0/".into())
        );
        assert_eq!(directory_scope(&[], "1.4.0"), Some("1.4.0/".into()));
    }

    /// A directory listing never trusts the loose server match: a copy of the path under
    /// another parent rides the scoped pages, and the client-side prefix filter drops it.
    #[tokio::test]
    async fn directory_listing_filters_the_loose_scope() {
        let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic).unwrap();
        let page = serde_json::json!({
            "items": [
                {"path": "/1.0.0/app.zip"},
                {"path": "/1.0.0/dir/tool.sh"},
                {"path": "/backup/1.0.0/decoy.zip"},
            ],
        });
        server.insert("service/rest/v1/search/assets", page.to_string().as_bytes());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cfg = crate::config::Config {
            base: server.base_url(),
            tls_insecure: false,
            ca_bundle: None,
            workers: 1,
            retry_attempts: 1,
            connect_timeout: std::time::Duration::from_secs(5),
            stall_timeout: std::time::Duration::from_secs(5),
            auth: None,
        };
        let client = NexusClient::new(&cfg, crate::events::Progress::new(tx)).unwrap();

        let (entries, _) = search_entries_counted(
            &client,
            &format!("{}/repository/raw/1.0.0/", server.base_url()),
        )
        .await
        .unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["dir", "app.zip"],
            "the copy under another parent stays out of the answer"
        );
    }

    /// The enumerator counts what it hides: a `.sha256` marker in the search
    /// page never appears as an entry and raises the hidden counter, which the
    /// CLI turns into a visible warning line.
    #[tokio::test]
    async fn search_entries_count_hides_markers_visibly() {
        let server = mock_nexus::MockNexus::start(mock_nexus::Scenario::Atomic).unwrap();
        let page = serde_json::json!({
            "items": [
                {"path": "1.0.0/app.zip"},
                {"path": "1.0.0/app.zip.sha256"},
                {"path": "1.0.0/needs.json"},
            ],
        });
        server.insert("service/rest/v1/search/assets", page.to_string().as_bytes());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let cfg = crate::config::Config {
            base: server.base_url(),
            tls_insecure: false,
            ca_bundle: None,
            workers: 1,
            retry_attempts: 1,
            connect_timeout: std::time::Duration::from_secs(5),
            stall_timeout: std::time::Duration::from_secs(5),
            auth: None,
        };
        let client = NexusClient::new(&cfg, crate::events::Progress::new(tx)).unwrap();

        let (entries, hidden) = search_entries_counted(
            &client,
            &format!("{}/repository/raw/1.0.0/", server.base_url()),
        )
        .await
        .unwrap();
        assert_eq!(hidden, 1, "one marker hidden");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["app.zip", "needs.json"], "no marker among entries");

        // The plain listing stays the same shape: the marker never surfaces.
        let plain = search_entries(
            &client,
            &format!("{}/repository/raw/1.0.0/", server.base_url()),
        )
        .await
        .unwrap();
        assert_eq!(plain.len(), 2);
    }

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
