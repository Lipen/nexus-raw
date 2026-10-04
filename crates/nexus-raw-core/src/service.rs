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
