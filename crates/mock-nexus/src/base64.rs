//! Hand-rolled standard base64 (RFC 4648) encoder.
//!
//! The only consumer is the `auth-401` scenario, which needs to build the expected `Authorization: Basic <b64(user:pass)>` token.
//! A ~20-line implementation beats an external crate here.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode `data` as standard base64 with `=` padding.
pub(crate) fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let triple = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(triple[0]) << 16) | (u32::from(triple[1]) << 8) | u32::from(triple[2]);
        let quad = [
            (n >> 18) & 0x3f,
            (n >> 12) & 0x3f,
            (n >> 6) & 0x3f,
            n & 0x3f,
        ];
        out.push(ALPHABET[quad[0] as usize] as char);
        out.push(ALPHABET[quad[1] as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[quad[2] as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[quad[3] as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn rfc4648_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encodes_default_credentials() {
        assert_eq!(encode(b"ci:secret"), "Y2k6c2VjcmV0");
    }
}
