use napi::Result;
use nexus_raw_core::{ArtifactName, Enumeration, Nxr};

use crate::error::js_error;

/// Resolve the enumeration source the way the CLI does: `ls` or explicit `names`.
pub(crate) fn resolve_enumeration(
    nxr: &Nxr,
    parsed_names: Option<Vec<ArtifactName>>,
    ls: bool,
    no_source_reason: &'static str,
) -> std::result::Result<Enumeration, nexus_raw_core::Error> {
    let _ = nxr;
    if ls {
        return Ok(Enumeration::Search);
    }
    if let Some(v) = parsed_names {
        return Ok(Enumeration::Names(v));
    }
    Err(nexus_raw_core::Error::Enumerate {
        url: nxr.base().to_owned(),
        reason: no_source_reason.to_owned(),
    })
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
