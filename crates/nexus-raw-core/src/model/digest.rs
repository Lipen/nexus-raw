//! Digest: sha256, streaming for files.

use std::fmt;
use std::io::Read;
use std::path::Path;

use sha2::{Digest as _, Sha256};

const HEX_LEN: usize = 64;
/// Streaming hash buffer.
const HASH_BUF: usize = 256 * 1024;

/// Hex digest string (64 lowercase hex chars).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest(String);

impl Digest {
    /// Parses a digest string.
    ///
    /// ```rust
    /// # use nexus_raw_core::Digest;
    /// let digest = Digest::of_bytes(b"hello");
    /// assert_eq!(digest.as_str(), "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    /// assert!(Digest::from_hex(digest.as_str()).is_ok());
    /// assert!(Digest::from_hex("DEADBEEF").is_err()); // lowercase only
    /// ```
    pub fn from_hex(s: &str) -> Result<Self, String> {
        if s.len() != HEX_LEN || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(format!("expected {HEX_LEN} lowercase hex chars, got {s:?}"));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(hex(&Sha256::digest(bytes)))
    }

    pub(crate) fn from_hex_string(s: String) -> Self {
        Self(s)
    }

    /// The "foreign object" digest used by the foreign-marker scenario.
    pub fn zero() -> Self {
        Self("0".repeat(HEX_LEN))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Streaming sha256 of a file with a 256 KiB buffer.
pub fn sha256_file(path: &Path) -> std::io::Result<Digest> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(Digest(hex(&hasher.finalize())))
}

/// A fresh hasher seeded with `bytes`, for incremental hashing.
pub fn sha256_bytes(bytes: &[u8]) -> Sha256 {
    let mut h = Sha256::new();
    h.update(bytes);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let d = Digest::of_bytes(b"hello");
        assert_eq!(
            d.as_str(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn rejects_uppercase_and_short() {
        assert!(Digest::from_hex(&"A".repeat(64)).is_err());
        assert!(Digest::from_hex("abc").is_err());
        assert!(Digest::from_hex(&"0".repeat(64)).is_ok());
    }
}
