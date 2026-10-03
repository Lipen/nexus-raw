//! Manifests: the enumeration source for `down` and the name filter for `up` (§5.2.1).

use std::path::Path;

use crate::error::Error;
use crate::model::name::ArtifactName;
use crate::transport::client::NexusClient;

/// A parsed manifest: artifact names in file order, duplicates removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub names: Vec<ArtifactName>,
}

impl Manifest {
    /// Parse a manifest: `{"artifacts": ["<name>", …]}`.
    ///
    /// The version-document fields are tolerated: `schema_version` must be 1 when present, `version` is ignored.
    /// Every name passes the grammar.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when the bytes are not a JSON object with an `artifacts` string array or carry an unsupported `schema_version`, and [`Error::UnsafeName`] when a name violates the grammar.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, Error> {
        let value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| Error::misuse(format!("manifest is not JSON: {e}")))?;
        let obj = value
            .as_object()
            .ok_or_else(|| Error::misuse("manifest must be a JSON object"))?;
        if let Some(v) = obj.get("schema_version") {
            let n = v
                .as_u64()
                .ok_or_else(|| Error::misuse("manifest schema_version must be a number"))?;
            if n != 1 {
                return Err(Error::misuse(format!(
                    "manifest schema_version {n} is not supported"
                )));
            }
        }
        let items = obj
            .get("artifacts")
            .and_then(|a| a.as_array())
            .ok_or_else(|| Error::misuse("manifest has no artifacts array"))?;
        let mut names = Vec::with_capacity(items.len());
        for item in items {
            let raw = item
                .as_str()
                .ok_or_else(|| Error::misuse("manifest artifact is not a string"))?;
            let name = ArtifactName::parse(raw)?;
            if !names.contains(&name) {
                names.push(name);
            }
        }
        Ok(Self { names })
    }

    /// Read a manifest from a local file.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the file cannot be read and the [`Self::from_slice`] parse errors.
    pub fn from_file(path: &Path) -> Result<Self, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::from_slice(&bytes)
    }

    /// Fetch a manifest from the server.
    ///
    /// # Errors
    ///
    /// Returns transport, auth or HTTP errors from the GET (a missing object is [`Error::Http`] 404) and the [`Self::from_slice`] parse errors.
    pub async fn from_url(client: &NexusClient, url: &str) -> Result<Self, Error> {
        match client.get_small(url).await? {
            None => Err(Error::Http {
                status: 404,
                url: url.to_owned(),
            }),
            Some(bytes) => Self::from_slice(&bytes),
        }
    }

    /// Read a manifest from stdin (`--manifest -`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Misuse`] when stdin cannot be read and the [`Self::from_slice`] parse errors.
    pub fn from_stdin() -> Result<Self, Error> {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| Error::misuse(format!("stdin: {e}")))?;
        Self::from_slice(&buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_version_shapes() {
        let m = Manifest::from_slice(br#"{"artifacts":["a.zip","sub/b.json"]}"#).unwrap();
        assert_eq!(m.names.len(), 2);
        let m2 = Manifest::from_slice(
            br#"{"schema_version":1,"version":"1.4.0","artifacts":["a.zip","a.zip"]}"#,
        )
        .unwrap();
        assert_eq!(m2.names.len(), 1);
    }

    #[test]
    fn rejects_bad_version_and_bad_names() {
        assert!(Manifest::from_slice(br#"{"schema_version":2,"artifacts":[]}"#).is_err());
        assert!(Manifest::from_slice(br#"{"artifacts":["x.sha256"]}"#).is_err());
        assert!(Manifest::from_slice(br#"{"names":[]}"#).is_err());
    }
}
