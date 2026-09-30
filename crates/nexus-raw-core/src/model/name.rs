//! Artifact name grammar, protocol §3.

use std::fmt;
use std::ops::Deref;

use crate::error::Error;

/// Artifact name: a path relative to the directory being transferred.
///
/// Validated on construction: segments `[A-Za-z0-9._-]+`, 1..=255 bytes each, no empty/`.`/`..` segments, no leading/trailing `/`.
/// Reserved: the `.sha256` suffix (marker collision).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactName(String);

const MAX_SEGMENT_BYTES: usize = 255;
const SIBLING_SUFFIX: &str = ".sha256";

fn valid_segment(seg: &str) -> bool {
    !seg.is_empty()
        && seg != "."
        && seg != ".."
        && seg.len() <= MAX_SEGMENT_BYTES
        && seg
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

impl ArtifactName {
    /// Parses and validates a name.
    ///
    /// ```rust
    /// # use nexus_raw_core::ArtifactName;
    /// let name = ArtifactName::parse("bom/linux-x86_64.json")?;
    /// assert_eq!(name.sibling(), "bom/linux-x86_64.json.sha256");
    /// assert_eq!(name.encoded(), "bom/linux-x86_64.json");
    /// assert!(ArtifactName::parse("x.sha256").is_err()); // reserved suffix
    /// # Ok::<(), nexus_raw_core::Error>(())
    /// ```
    pub fn parse(name: &str) -> Result<Self, Error> {
        let err = |reason: &str| Error::UnsafeName {
            name: name.to_owned(),
            reason: reason.to_owned(),
        };
        if name.is_empty() {
            return Err(err("empty name"));
        }
        if name.starts_with('/') {
            return Err(err("leading '/'"));
        }
        if name.ends_with('/') {
            return Err(err("trailing '/'"));
        }
        if name.ends_with(SIBLING_SUFFIX) {
            return Err(err("reserved suffix .sha256 (artifact/marker collision)"));
        }
        for seg in name.split('/') {
            if !valid_segment(seg) {
                return Err(err(
                    "each segment must match [A-Za-z0-9._-]+, be 1..=255 bytes, and not be '.' or '..'",
                ));
            }
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The sha-sibling name: `<name>.sha256`.
    pub fn sibling(&self) -> String {
        format!("{name}{SIBLING_SUFFIX}", name = self.0)
    }

    /// Percent-encoding per segment.
    /// `/` stays the path separator (§3).
    pub fn encoded(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut out = String::with_capacity(self.0.len());
        for seg in self.0.split('/') {
            if !out.is_empty() {
                out.push('/');
            }
            for &b in seg.as_bytes() {
                match b {
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'-' | b'~' => {
                        out.push(b as char)
                    }
                    _ => {
                        out.push('%');
                        out.push(HEX[(b >> 4) as usize] as char);
                        out.push(HEX[(b & 0x0f) as usize] as char);
                    }
                }
            }
        }
        out
    }
}

impl fmt::Display for ArtifactName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Deref for ArtifactName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// Version segment validation (§4.1 sane check): same grammar as a name segment.
pub fn validate_version(version: &str) -> Result<(), Error> {
    if valid_segment(version) {
        Ok(())
    } else {
        Err(Error::misuse(format!(
            "invalid version segment {version:?}: must match [A-Za-z0-9._-]+, 1..=255 bytes, not '.'/'..'"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unsafe_reason(name: &str) -> String {
        match ArtifactName::parse(name) {
            Ok(_) => panic!("{name:?} must be rejected"),
            Err(Error::UnsafeName { reason, .. }) => reason,
            Err(e) => panic!("{name:?}: unexpected error {e}"),
        }
    }

    #[test]
    fn accepts_valid_names() {
        for name in ["pinned.xml", "bom/linux-x86_64.json", "a-b.c_d"] {
            ArtifactName::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn rejects_dangerous_names() {
        assert!(!unsafe_reason("..").is_empty());
        assert!(!unsafe_reason("/abs").is_empty());
        assert!(!unsafe_reason("a//b").is_empty());
        assert!(!unsafe_reason("x/").is_empty());
        assert!(!unsafe_reason("x.sha256").is_empty());
        // No reserved names beyond the .sha256 suffix.
        // `latest`/`nightly`/`version.json` are ordinary artifact names: channels and manifests are generic files, not protocol.
        for name in ["latest", "nightly", "version.json"] {
            ArtifactName::parse(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert!(!unsafe_reason("~x").is_empty());
        assert!(!unsafe_reason("a/../b").is_empty());
        assert!(!unsafe_reason("a/./b").is_empty());
        assert!(!unsafe_reason("\u{43f}\u{440}\u{438}\u{432}\u{435}\u{442}").is_empty());
        assert!(!unsafe_reason("a b").is_empty());
    }

    #[test]
    fn rejects_overlong_segment() {
        let long = "a".repeat(256);
        assert!(!unsafe_reason(&long).is_empty());
        let ok = "a".repeat(255);
        ArtifactName::parse(&ok).expect("255 bytes is valid");
    }

    #[test]
    fn encodes_per_segment() {
        let n = ArtifactName::parse("dir/name-1.tar.gz").unwrap();
        assert_eq!(n.encoded(), "dir/name-1.tar.gz");
    }

    #[test]
    fn sibling_name() {
        let n = ArtifactName::parse("a.zip").unwrap();
        assert_eq!(n.sibling(), "a.zip.sha256");
    }

    #[test]
    fn version_validation() {
        validate_version("1.14.0").unwrap();
        validate_version("nightly-2026.09.25").unwrap();
        assert!(validate_version("").is_err());
        assert!(validate_version("../etc").is_err());
        assert!(validate_version("a b").is_err());
    }
}
