//! Sha-sibling: the `sha256sum -c` line format (protocol §4.2).

use super::digest::Digest;

/// Exactly one line `<hex>  <name>\n`: lowercase hex, two spaces, trailing `\n`.
pub fn format_line(name: &str, digest: &Digest) -> String {
    format!("{}  {name}\n", digest.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sibling {
    pub digest: Digest,
    pub name: String,
}

/// Strict single-line parse: no CRLF, no extra lines, exactly two spaces.
pub fn parse_line(s: &str) -> Result<Sibling, String> {
    let body = s.strip_suffix('\n').ok_or("missing trailing newline")?;
    if body.contains('\n') {
        return Err("expected exactly one line".into());
    }
    if body.contains('\r') {
        return Err("CR found: CRLF is not allowed".into());
    }
    let (hex, name) = body
        .split_once("  ")
        .ok_or("two-space separator not found")?;
    if name.is_empty() || name.contains("  ") || name.starts_with(' ') {
        return Err("name must be non-empty with exactly two spaces before it".into());
    }
    let digest = Digest::from_hex(hex).map_err(|e| format!("bad digest: {e}"))?;
    Ok(Sibling {
        digest,
        name: name.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  a.zip\n";

    #[test]
    fn roundtrip() {
        let parsed = parse_line(OK).unwrap();
        assert_eq!(parsed.name, "a.zip");
        assert_eq!(format_line("a.zip", &parsed.digest), OK);
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
