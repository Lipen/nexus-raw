use std::path::Path;

use napi::Result;
use nexus_raw_core::{ArtifactName, Enumeration, Manifest, Nxr};

use crate::error::js_error;
use crate::mapping;

/// Resolve the enumeration source the way the CLI does: `ls`, a `manifest` spec, explicit `names`, or the conventional `manifest.json` at the directory URL.
/// The caller must drop the facade before draining the pump on the error path.
pub(crate) async fn resolve_enumeration(
    nxr: &Nxr,
    manifest: &Option<String>,
    parsed_names: Option<Vec<ArtifactName>>,
    ls: bool,
    no_manifest_reason: &'static str,
) -> std::result::Result<Enumeration, nexus_raw_core::Error> {
    if ls {
        return Ok(Enumeration::Search);
    }
    if let Some(spec) = manifest {
        return load_manifest(nxr, spec).await.map(Enumeration::Manifest);
    }
    if let Some(v) = parsed_names {
        return Ok(Enumeration::Names(v));
    }
    match nxr.manifest_at_base().await {
        Ok(Some(m)) => Ok(Enumeration::Manifest(m)),
        Ok(None) => Err(nexus_raw_core::Error::Enumerate {
            url: nxr.base().to_owned(),
            reason: no_manifest_reason.to_owned(),
        }),
        Err(e) => Err(e),
    }
}

/// Resolve a `manifest` spec: `-` for stdin, http(s) URLs through the server, everything else as a local file — the CLI rules.
pub(crate) async fn load_manifest(
    nxr: &Nxr,
    spec: &str,
) -> std::result::Result<Manifest, nexus_raw_core::Error> {
    match mapping::classify_manifest_spec(spec) {
        mapping::ManifestSpec::Stdin => Manifest::from_stdin(),
        mapping::ManifestSpec::Url(url) => nxr.manifest_from(&url).await,
        mapping::ManifestSpec::File(path) => Manifest::from_file(Path::new(&path)),
    }
}

/// Parse explicit names through the grammar (exit 2 on bad names).
pub(crate) fn parse_names(names: Option<Vec<String>>) -> Result<Option<Vec<ArtifactName>>> {
    match names {
        None => Ok(None),
        // An explicit empty list restricts nothing: that is a caller bug, not a whole-catalog request.
        Some(v) if v.is_empty() => Err(js_error(nexus_raw_core::Error::Misuse(
            "names: an empty list would transfer the whole catalog; omit the \
             option for that, or name at least one artifact"
                .to_owned(),
        ))),
        Some(v) => v
            .iter()
            .map(|s| ArtifactName::parse(s).map_err(js_error))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_names;

    #[test]
    fn empty_names_list_is_misuse_not_whole_catalog() {
        assert!(parse_names(Some(vec![])).is_err());
        assert!(parse_names(None).unwrap().is_none());
        assert_eq!(
            parse_names(Some(vec!["a.zip".into()]))
                .unwrap()
                .unwrap()
                .len(),
            1
        );
    }
}
