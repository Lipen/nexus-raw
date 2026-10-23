//! The audit report: the integrity classes of one raw repository (`nxr audit`).
//!
//! The classification is a pure function over metadata facts: the raw tree
//! listing (markers included), the marker bodies and the search facts.
//! Asset bytes are never read here. The `--bytes` mode lives in the executor
//! and feeds the same finding type.
//! The repository is only ever read: no audit request mutates it.

use std::collections::BTreeMap;

use crate::model::digest::Digest;
use crate::model::name::ArtifactName;

/// One integrity finding: a name the metadata cannot vouch for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The class the facts put the name in.
    pub class: FindingClass,
    /// The name the class attaches to: an asset name for most classes, the
    /// marker's own name for a dangling or unparseable marker.
    pub name: ArtifactName,
    /// The human detail: what exactly the class saw.
    pub detail: String,
}

/// The integrity class of one finding.
///
/// Serde names are the report's vocabulary (`finding` events, `--json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingClass {
    /// Bytes exist, the marker does not: a consumer cannot verify the copy.
    Orphan,
    /// A marker exists, the bytes do not: a consumer gets a 404 behind a name
    /// the marker listing still has.
    DanglingMarker,
    /// The marker body is outside the `sha256sum -c` format: nothing to
    /// verify against.
    UnparseableMarker,
    /// The marker digest and the search fact disagree: the copy or the index
    /// lies, and the audit cannot say which.
    DivergedFact,
}

impl FindingClass {
    /// The serde (and human) name of the class.
    pub fn as_str(self) -> &'static str {
        match self {
            FindingClass::Orphan => "orphan",
            FindingClass::DanglingMarker => "dangling_marker",
            FindingClass::UnparseableMarker => "unparseable_marker",
            FindingClass::DivergedFact => "diverged_fact",
        }
    }
}

/// The digest line of a marker body: `<hex digest>  <name>`, `sha256sum -c` style.
///
/// The name is not re-parsed as an artifact name here: a foreign or hand-edited
/// marker may carry anything, and the audit reports it, never refuses it.
pub fn parse_marker_body(body: &str) -> Option<(Digest, String)> {
    let line = body.lines().next()?;
    let mut parts = line.splitn(2, "  ");
    let hex = parts.next()?.trim();
    let named = parts.next()?.trim();
    if named.is_empty() {
        return None;
    }
    let digest = Digest::from_hex(hex).ok()?;
    Some((digest, named.to_owned()))
}

/// The inputs of one structural audit run, in authority order.
///
/// - `tree`: the raw tree listing, markers included, as name -> the marker body
///   when the marker is one of the listed objects. The tree is the truth about
///   what exists: the search index lags behind uploads and never feeds the
///   existence verdicts.
/// - `facts`: the search-reported digest per asset name, when the index is
///   alive. Absence of a fact is absence of a cross-check, never a finding.
#[derive(Debug, Clone, Default)]
pub struct AuditInput {
    pub tree: BTreeMap<String, Option<String>>,
    pub facts: BTreeMap<ArtifactName, Option<Digest>>,
}

/// The structural verdict: findings per class plus whether the search facts
/// took part.
///
/// The verdict is honest about a dead index: without facts the cross-check
/// (`diverged_fact`) cannot run, and the report says so instead of staying
/// silent about the gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditVerdict {
    pub findings: Vec<Finding>,
    pub facts_available: bool,
}

/// The structural classification over the audit input.
///
/// Per listed object:
/// - an asset without its marker is an [`FindingClass::Orphan`].
/// - a marker whose asset is not listed is a [`FindingClass::DanglingMarker`].
/// - a marker body outside the format is an [`FindingClass::UnparseableMarker`].
/// - a parsed marker whose digest disagrees with the search fact is a
///   [`FindingClass::DivergedFact`].
///
/// Everything else is clean: the audit reports problems, not inventories.
pub fn classify(input: &AuditInput) -> AuditVerdict {
    let mut findings = Vec::new();
    let facts_available = !input.facts.is_empty();
    // The tree splits into assets and markers, and each side checks the other.
    let markers: Vec<&String> = input
        .tree
        .keys()
        .filter(|n| n.ends_with(".sha256"))
        .collect();
    let assets: Vec<&String> = input
        .tree
        .keys()
        .filter(|n| !n.ends_with(".sha256"))
        .collect();

    for asset in &assets {
        let Ok(asset_name) = ArtifactName::parse(asset) else {
            continue;
        };
        let marker_key = format!("{}.sha256", asset);
        if !input.tree.contains_key(&marker_key) {
            findings.push(Finding {
                class: FindingClass::Orphan,
                name: asset_name,
                detail: "bytes without marker".to_owned(),
            });
        }
    }
    for marker in &markers {
        let body = input.tree.get(*marker).cloned().flatten();
        let asset_str = marker.strip_suffix(".sha256");
        let Some(asset_str) = asset_str else {
            continue;
        };
        let Some(asset) = ArtifactName::parse(asset_str).ok() else {
            // The marker maps to no legal asset name: report the marker
            // itself under the closest parseable form of its asset half, or
            // skip when even the segment grammar refuses it.
            let Ok(nominal) =
                ArtifactName::parse(asset_str.split('/').next_back().unwrap_or("marker"))
            else {
                continue;
            };
            findings.push(Finding {
                class: FindingClass::UnparseableMarker,
                name: nominal,
                detail: format!("marker name outside the name grammar: {}", marker),
            });
            continue;
        };
        if !input.tree.contains_key(asset_str) {
            findings.push(Finding {
                class: FindingClass::DanglingMarker,
                name: asset,
                detail: "marker without bytes".to_owned(),
            });
            continue;
        }
        let Some(body) = body else {
            // The executor fetched every listed marker. An absent body means
            // the marker was filtered before the fetch (name filters), so
            // this pair was never checked: silence, not a finding.
            continue;
        };
        let Some((digest, _)) = parse_marker_body(&body) else {
            findings.push(Finding {
                class: FindingClass::UnparseableMarker,
                name: asset,
                detail: "marker body outside the digest format".to_owned(),
            });
            continue;
        };
        if let Some(Some(fact)) = input.facts.get(&asset) {
            if fact != &digest {
                findings.push(Finding {
                    class: FindingClass::DivergedFact,
                    name: asset.clone(),
                    detail: format!("marker says {}, the search index says {}", digest, fact),
                });
            }
        }
    }
    findings.sort_by(|a, b| (&a.class, &a.name).cmp(&(&b.class, &b.name)));
    AuditVerdict {
        findings,
        facts_available,
    }
}

/// The byte-level verdict for `--bytes`: the digest of the downloaded bytes
/// against the marker digest.
///
/// This is the only check that sees silent bitrot: the metadata above stays
/// blind to a byte that changed on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteCheck {
    /// The downloaded digest matches the marker.
    Match,
    /// The digests differ: the bytes on disk are not what the marker vouches for.
    Mismatch {
        /// The digest the marker carries.
        marker: Digest,
        /// The digest the downloaded bytes hash to.
        actual: Digest,
    },
}

/// The client-side narrowing of an audit: exact names and whole-segment
/// prefixes, with the same grammar every transfer filter uses.
/// An empty filter audits the whole tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    /// Whole-segment prefixes (`bom/`).
    pub prefixes: Vec<crate::model::name::NamePrefix>,
    /// Exact object names.
    pub names: Vec<ArtifactName>,
}

impl AuditFilter {
    /// Whether the filter selects everything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty() && self.names.is_empty()
    }

    /// Whether an object name passes the filter.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        (self.names.is_empty() || self.names.iter().any(|n| n.as_str() == name))
            && (self.prefixes.is_empty()
                || self.prefixes.iter().any(|p| name.starts_with(p.as_str())))
    }

    /// Whether a raw tree key passes the filter.
    /// A marker key is judged by the object it marks: selecting a name never
    /// leaves its sibling behind, which would report every audited asset as
    /// an orphan.
    #[must_use]
    pub fn matches_key(&self, key: &str) -> bool {
        self.matches(key.strip_suffix(".sha256").unwrap_or(key))
    }
}

/// One full audit run: the structural verdict plus the byte pass when asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRun {
    /// The structural verdict over tree, markers and facts.
    pub verdict: AuditVerdict,
    /// The number of non-marker objects the tree listing carried: the
    /// denominator of `findings`, whether or not the byte pass ran.
    pub audited: usize,
    /// The byte pass result per checked asset, when `bytes_mode` ran.
    pub byte_checks: BTreeMap<ArtifactName, ByteCheck>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> ArtifactName {
        ArtifactName::parse(s).expect("valid name")
    }

    fn tree_entry(k: &str, body: Option<&str>) -> (String, Option<String>) {
        (k.to_owned(), body.map(str::to_owned))
    }

    #[test]
    fn orphan_and_clean_pair() {
        let input = AuditInput {
            tree: [
                tree_entry("app.zip", None),
                tree_entry("lib.zip.sha256", Some("aa  lib.zip\n")),
            ]
            .into(),
            facts: BTreeMap::new(),
        };
        let v = classify(&input);
        assert!(!v.facts_available);
        assert_eq!(v.findings.len(), 2);
        assert_eq!(v.findings[0].class, FindingClass::Orphan);
        assert_eq!(v.findings[1].class, FindingClass::DanglingMarker);
    }

    #[test]
    fn unparseable_body_is_a_class_not_an_error() {
        let input = AuditInput {
            tree: [
                tree_entry("app.zip", None),
                tree_entry("app.zip.sha256", Some("garbage line\n")),
            ]
            .into(),
            facts: BTreeMap::new(),
        };
        let v = classify(&input);
        assert_eq!(v.findings.len(), 1);
        assert_eq!(v.findings[0].class, FindingClass::UnparseableMarker);
    }

    #[test]
    fn diverged_fact_needs_two_digests() {
        let d1 = Digest::from_hex(&"a".repeat(64)).expect("digest");
        let d2 = Digest::from_hex(&"b".repeat(64)).expect("digest");
        let input = AuditInput {
            tree: [
                tree_entry("app.zip", None),
                tree_entry("app.zip.sha256", Some(&format!("{}  app.zip\n", d1))),
            ]
            .into(),
            facts: [(name("app.zip"), Some(d2))].into_iter().collect(),
        };
        let v = classify(&input);
        assert!(v.facts_available);
        assert_eq!(v.findings.len(), 1);
        assert_eq!(v.findings[0].class, FindingClass::DivergedFact);
    }

    #[test]
    fn fresh_upload_missing_fact_is_clean() {
        let d1 = Digest::from_hex(&"a".repeat(64)).expect("digest");
        let input = AuditInput {
            tree: [
                tree_entry("app.zip", None),
                tree_entry("app.zip.sha256", Some(&format!("{}  app.zip\n", d1))),
            ]
            .into(),
            facts: [(name("other.zip"), Some(d1.clone()))]
                .into_iter()
                .collect(),
        };
        let v = classify(&input);
        assert!(v.findings.is_empty());
    }
}
