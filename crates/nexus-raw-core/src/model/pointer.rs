//! Pointer files: the token format and the dotted-numeric comparison (§4.3, §6.3).

/// A version token: one line, trailing newline, non-empty, never CRLF.
pub fn parse_token(raw: &str) -> Result<String, String> {
    let body = raw.strip_suffix('\n').ok_or("missing trailing newline")?;
    if body.contains('\r') {
        return Err("CR found: CRLF is not allowed".into());
    }
    if body.contains('\n') {
        return Err("expected exactly one line".into());
    }
    if body.is_empty() {
        return Err("empty pointer token".into());
    }
    Ok(body.to_owned())
}

/// The pointer body: token + `\n`.
#[must_use]
pub fn format_token(version: &str) -> String {
    format!("{version}\n")
}

/// A token must be exactly one non-empty line, no CR.
pub fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty() {
        return Err("empty token".into());
    }
    if token.contains('\n') {
        return Err("expected exactly one line".into());
    }
    if token.contains('\r') {
        return Err("CR found: CRLF is not allowed".into());
    }
    Ok(())
}

/// `a >= b`, dotted-numeric: numeric chunks as numbers, the rest lexicographically.
///
/// A segment is (numeric prefix, suffix): a release is newer than its own
/// prerelease, so `1.0.0-rc1` cannot roll a pointer back from `1.0.0`.
///
/// ```rust
/// use nexus_raw_core::model::pointer::version_ge;
/// assert!(version_ge("1.10.0", "1.9.9"));
/// assert!(version_ge("1.0.0", "1.0.0-rc1"));
/// assert!(!version_ge("1.4.0", "1.14.0"));
/// ```
#[must_use]
pub fn version_ge(a: &str, b: &str) -> bool {
    compare(a, b) != std::cmp::Ordering::Less
}

fn compare(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ia = a.split('.');
    let mut ib = b.split('.');
    loop {
        match (ia.next(), ib.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = compare_segment(x, y);
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// A segment is (numeric prefix, suffix).
///
/// Prefixes compare as numbers.
/// On a tie the release (`"0"`) is newer than its prerelease (`"0-rc1"`).
/// Remaining suffixes compare lexicographically.
fn compare_segment(x: &str, y: &str) -> std::cmp::Ordering {
    let (xn, xs) = split_num_prefix(x);
    let (yn, ys) = split_num_prefix(y);
    compare_numeric(xn, yn).then_with(|| match (xs.is_empty(), ys.is_empty()) {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        (false, false) => xs.cmp(ys),
    })
}

fn split_num_prefix(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at(end)
}

fn compare_numeric(x: &str, y: &str) -> std::cmp::Ordering {
    let x = x.trim_start_matches('0');
    let y = y.trim_start_matches('0');
    x.len().cmp(&y.len()).then_with(|| x.cmp(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_roundtrip() {
        assert_eq!(parse_token("1.14.0\n").unwrap(), "1.14.0");
        assert!(parse_token("1.14.0").is_err());
        assert!(parse_token("1.14.0\r\n").is_err());
        assert!(parse_token("\n").is_err());
        assert!(parse_token("a\nb\n").is_err());
    }

    #[test]
    fn dotted_numeric_ordering() {
        assert!(version_ge("1.14.0", "1.14.0"));
        assert!(version_ge("1.14.1", "1.14.0"));
        assert!(!version_ge("1.14.0", "1.14.1"));
        assert!(!version_ge("1.9.0", "1.10.0"));
        assert!(version_ge("1.10.0", "1.9.9"));
        assert!(version_ge("nightly-2026.09.25", "nightly-2026.09.24"));
        assert!(!version_ge("nightly-2026.09.24", "nightly-2026.09.25"));
        assert!(version_ge("1.14", "1.13.9"));
        assert!(!version_ge("1.13", "1.13.0"));
        assert!(version_ge("1.0.0", "1.0.0-rc1"));
        assert!(!version_ge("1.0.0-rc1", "1.0.0"));
        assert!(version_ge("2.0", "1.99.99"));
    }
}
