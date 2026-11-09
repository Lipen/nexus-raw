//! Sha-sibling: the `sha256sum -c` line format (protocol §4.2).

use super::digest::Digest;

/// Exactly one line `<hex>  <name>\n`: lowercase hex, two spaces, trailing `\n`.
///
/// ```rust
/// use nexus_raw_core::model::sibling;
/// use nexus_raw_core::Digest;
/// let digest = Digest::of_bytes(b"payload");
/// let line = sibling::format_line("a.zip", &digest);
/// assert_eq!(line, format!("{digest}  a.zip\n"));
/// assert_eq!(sibling::parse_line(&line)?.digest, digest);
/// # Ok::<(), String>(())
/// ```
#[must_use]
pub fn format_line(name: &str, digest: &Digest) -> String {
    format!("{}  {name}\n", digest.as_str())
}

/// Write the canonical marker beside its bytes, with the policy every transfer
/// shares: the file is rewritten in place and never opened through a symlink, so
/// a link planted at the marker path cannot receive it.
///
/// # Errors
///
/// Returns [`Error::Io`](crate::error::Error::Io) when the open, the write or
/// the flush fails.
#[cfg(not(target_arch = "wasm32"))]
pub async fn write_marker(
    path: &std::path::Path,
    name: &str,
    digest: &Digest,
) -> Result<(), crate::error::Error> {
    use crate::error::Error;
    let line = format_line(name, digest);
    let mut file = crate::transport::client::write_options(false)
        .open(path)
        .await
        .map_err(|e| Error::io(path, e))?;
    use tokio::io::AsyncWriteExt as _;
    file.write_all(line.as_bytes())
        .await
        .map_err(|e| Error::io(path, e))?;
    file.flush().await.map_err(|e| Error::io(path, e))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub digest: Digest,
    /// The artifact the line names, `None` for a bare checksum line that carries the digest alone.
    pub name: Option<String>,
}

/// Single-line parse: no CRLF, no extra lines, and one of the two shapes a sibling comes in.
///
/// The canonical `sha256sum -c` line names its artifact, and a Maven- or Gradle-style
/// checksum file carries the digest alone. Both identify the bytes, so both are read;
/// a writer always produces the canonical line.
///
/// # Errors
///
/// Returns the reason string when the line format is violated or the digest is not 64 lowercase hex characters.
pub fn parse_line(s: &str) -> Result<Sibling, String> {
    let body = s.strip_suffix('\n').ok_or("missing trailing newline")?;
    if body.contains('\n') {
        return Err("expected exactly one line".into());
    }
    if body.contains('\r') {
        return Err("CR found: CRLF is not allowed".into());
    }
    let Some((hex, name)) = body.split_once("  ") else {
        let digest = Digest::from_hex(body)
            .map_err(|e| format!("bad digest: {e} (a bare line is read as a checksum file)"))?;
        return Ok(Sibling { digest, name: None });
    };
    if name.is_empty() || name.contains("  ") || name.starts_with(' ') {
        return Err("name must be non-empty with exactly two spaces before it".into());
    }
    let digest = Digest::from_hex(hex).map_err(|e| format!("bad digest: {e}"))?;
    Ok(Sibling {
        digest,
        name: Some(name.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  a.zip\n";

    #[test]
    fn roundtrip() {
        let parsed = parse_line(OK).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("a.zip"));
        assert_eq!(format_line("a.zip", &parsed.digest), OK);
    }

    /// The common Maven-style checksum: the digest alone, no name. Read, with the name absent.
    #[test]
    fn a_bare_digest_is_read() {
        let bare = format!("{}\n", "0".repeat(64));
        let parsed = parse_line(&bare).unwrap();
        assert!(parsed.name.is_none(), "a bare line names no artifact");
        assert_eq!(parsed.digest.as_str(), "0".repeat(64));
    }

    /// A bare line still obeys the digest rules: uppercase and short forms are refused.
    #[test]
    fn a_bare_line_keeps_the_digest_rules() {
        let upper = format!("{}\n", "A".repeat(64));
        assert!(parse_line(&upper).is_err(), "uppercase hex is refused");
        assert!(
            parse_line("deadbeef\n").is_err(),
            "a short digest is refused"
        );
        assert!(parse_line(&format!("{}\r\n", "0".repeat(64))).is_err());
    }

    #[test]
    fn rejects_foreign_formats() {
        // no trailing \n
        assert!(parse_line(OK.trim_end()).is_err());
        // CRLF
        assert!(parse_line(&format!("{}\r", OK.trim_end())).is_err());
        // single space
        assert!(parse_line(&OK.replace("  ", " ")).is_err());
        // three spaces
        assert!(parse_line(&OK.replace("  ", "   ")).is_err());
        // two lines
        assert!(parse_line(&format!("{OK}{OK}")).is_err());
        // garbage instead of hex
        assert!(parse_line(&format!("zz{}  a.zip\n", "0".repeat(62))).is_err());
        // uppercase hex
        assert!(parse_line(&OK.to_uppercase().replace(".ZIP", ".zip")).is_err());
        // empty name
        let empty_name = format!("{}  \n", "0".repeat(64));
        assert!(parse_line(&empty_name).is_err());
    }
}
