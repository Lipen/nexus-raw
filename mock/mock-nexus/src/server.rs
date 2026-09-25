//! Minimal HTTP/1.1 request parsing and response writing (std only).
//!
//! One request per connection: read the head, decide how much of the body to
//! take (a scenario may cut the body short), then write a single response
//! carrying `Content-Length` and `Connection: close`.

use std::io::{self, BufRead, Read, Write};
use std::thread;
use std::time::Duration;

/// Upper bound for the request head; anything larger is answered with 400.
const HEAD_BOUND: usize = 64 * 1024;

/// A parsed HTTP/1.1 request.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) method: String,
    /// Raw request target as sent (percent-encoding preserved).
    pub(crate) target: String,
    /// Headers in arrival order, `(name, value)` with surrounding whitespace trimmed.
    pub(crate) headers: Vec<(String, String)>,
    /// Fully read request body (empty unless the request declared one).
    pub(crate) body: Vec<u8>,
}

impl Request {
    /// Case-insensitive header lookup; returns the first match.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// How much of the request body to read before treating the request as done.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BodyPlan {
    /// Read the full body as declared by `Content-Length` / chunked framing.
    Full,
    /// Read at most `n` body bytes and give up on the request (partial-put).
    Partial(usize),
}

/// Why a request body could not be read to completion.
#[derive(Debug)]
pub(crate) enum BodyError {
    /// The framing itself was broken; the peer still gets a 400.
    Malformed,
    /// The peer aborted (or we cut) the body; `usize` is the byte count received.
    Aborted(usize),
}

/// An HTTP response ready to serialize.
#[derive(Debug)]
pub(crate) struct Resp {
    pub(crate) status: u16,
    pub(crate) extra_headers: Vec<(&'static str, String)>,
    /// Value sent as `Content-Length` (the real body length even for HEAD,
    /// which writes no body bytes).
    pub(crate) content_length: usize,
    /// Bytes to write on the wire (empty for HEAD).
    pub(crate) body: Vec<u8>,
    /// Slow-drip parameters; `Some` only for the `slow` scenario.
    pub(crate) drip: Option<Drip>,
}

/// Slow-drip parameters simulating a low-bandwidth link.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drip {
    pub(crate) delay_ms: u64,
    pub(crate) chunk_size: usize,
}

/// Read the request head with byte-precise control: stop as soon as the
/// buffer ends with the first three bytes of the CRLFCRLF terminator,
/// deliberately leaving the final byte unread on the socket.
///
/// The drop-connection scenario exploits this: a socket closed while its
/// receive queue still holds data is reset (RST) by the kernel instead of
/// being closed cleanly, so the peer sees a connection reset, not EOF.
pub(crate) fn read_head_raw<R: Read>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = r.read(&mut byte)?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed mid-head",
                ))
            };
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r") {
            return Ok(Some(buf));
        }
        if buf.len() > HEAD_BOUND {
            return Err(malformed("request head too large"));
        }
    }
}

/// Parse request line and headers from head bytes as produced by
/// [`read_head_raw`] (which stop short of the terminator's final `\n`).
pub(crate) fn parse_head(head: &[u8]) -> io::Result<Request> {
    let mut full = head.to_vec();
    full.push(b'\n');
    let text = String::from_utf8_lossy(&full);
    let mut lines = text.split("\r\n");

    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || method.is_empty()
        || target.is_empty()
        || !version.starts_with("HTTP/")
    {
        return Err(malformed("malformed request line"));
    }

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(malformed("malformed header line"));
        };
        headers.push((name.trim().to_owned(), value.trim().to_owned()));
    }

    Ok(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body: Vec::new(),
    })
}

/// Read the request body into `req.body` according to `plan`.
///
/// On `BodyError::Aborted` the body holds whatever bytes arrived before the
/// connection broke; the request is never complete in that case.
pub(crate) fn read_body_into<R: BufRead>(
    r: &mut R,
    req: &mut Request,
    plan: BodyPlan,
) -> Result<(), BodyError> {
    if let BodyPlan::Partial(cut) = plan {
        // Cut mid-body regardless of framing: the request will never complete.
        // Clamp to the declared length so short bodies cannot stall us.
        let declared = req
            .header("content-length")
            .and_then(|v| v.parse::<usize>().ok());
        let take = cut.min(declared.unwrap_or(cut));
        let mut sink = Vec::new();
        let _ = read_exact_buf(r, &mut sink, take);
        return Err(BodyError::Aborted(sink.len()));
    }

    let chunked = req
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let result = if chunked {
        read_chunked(r, req)
    } else if let Some(len) = req.header("content-length") {
        match len.parse::<usize>() {
            Ok(n) => read_exact_buf(r, &mut req.body, n).map(|_| ()),
            Err(_) => Err(malformed("invalid content-length")),
        }
    } else {
        Ok(())
    };
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => Err(BodyError::Malformed),
        Err(_) => Err(BodyError::Aborted(req.body.len())),
    }
}

/// Serialize and write one response. Always sends `Content-Length` and
/// `Connection: close`; the caller drops the stream afterwards.
pub(crate) fn write_response<W: Write>(w: &mut W, resp: &Resp) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status,
        reason(resp.status),
        resp.content_length
    );
    for (name, value) in &resp.extra_headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;

    match resp.drip {
        Some(drip) if !resp.body.is_empty() => {
            for (i, piece) in resp.body.chunks(drip.chunk_size.max(1)).enumerate() {
                if i > 0 {
                    thread::sleep(Duration::from_millis(drip.delay_ms));
                }
                w.write_all(piece)?;
                w.flush()?;
            }
        }
        _ => w.write_all(&resp.body)?,
    }
    w.flush()
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        503 => "Service Unavailable",
        _ => "",
    }
}

/// Read exactly `n` more bytes, appending them to `out`.
fn read_exact_buf<R: BufRead>(r: &mut R, out: &mut Vec<u8>, n: usize) -> io::Result<()> {
    out.reserve(n);
    let mut remaining = n;
    while remaining > 0 {
        let available = r.fill_buf()?;
        if available.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed mid-body",
            ));
        }
        let take = remaining.min(available.len());
        out.extend_from_slice(&available[..take]);
        r.consume(take);
        remaining -= take;
    }
    Ok(())
}

/// Decode a chunked body (RFC 9112 §7.1) into `req.body`, including trailers.
fn read_chunked<R: BufRead>(r: &mut R, req: &mut Request) -> io::Result<()> {
    loop {
        let size_line = read_crlf_line(r)?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| malformed("bad chunk size"))?;
        if size == 0 {
            // Trailer section: header lines up to the terminating empty line.
            loop {
                if read_crlf_line(r)?.is_empty() {
                    return Ok(());
                }
            }
        }
        read_exact_buf(r, &mut req.body, size)?;
        let mut tail = [0u8; 2];
        r.read_exact(&mut tail)?;
        if &tail != b"\r\n" {
            return Err(malformed("chunk data not CRLF-terminated"));
        }
    }
}

/// Read one CRLF-terminated line as text (without the line ending).
fn read_crlf_line<R: BufRead>(r: &mut R) -> io::Result<String> {
    let mut raw = Vec::new();
    let read = r.read_until(b'\n', &mut raw)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "connection closed mid-body",
        ));
    }
    while matches!(raw.last(), Some(b'\n' | b'\r')) {
        raw.pop();
    }
    String::from_utf8(raw).map_err(|_| malformed("non-utf8 chunk framing"))
}

fn malformed(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        parse_head, read_body_into, read_head_raw, write_response, BodyError, BodyPlan, Request,
        Resp,
    };
    use std::io::Read;

    /// Run the production head + body path over an in-memory stream.
    fn parse(raw: &[u8]) -> (Vec<u8>, Request) {
        let mut r: &[u8] = raw;
        let head = read_head_raw(&mut r).expect("head reads").expect("not eof");
        let mut req = parse_head(&head).expect("head parses");
        // Consume the reserved terminator byte, as serve() does, then the body.
        let mut reserved = [0u8; 1];
        r.read_exact(&mut reserved).expect("reserved byte");
        assert_eq!(reserved[0], b'\n');
        read_body_into(&mut r, &mut req, BodyPlan::Full).expect("body reads");
        (r.to_vec(), req)
    }

    #[test]
    fn parses_content_length_body() {
        let (rest, req) =
            parse(b"PUT /a%20b HTTP/1.1\r\nContent-Length: 5\r\nX-Bernard: 1\r\n\r\nhello");
        assert!(rest.is_empty());
        assert_eq!(req.method, "PUT");
        assert_eq!(req.target, "/a%20b");
        assert_eq!(req.body, b"hello");
        assert_eq!(req.header("x-BERNARD"), Some("1"));
    }

    #[test]
    fn parses_chunked_body_with_trailers() {
        let raw = b"POST /c HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                    4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\nX-Trailer: t\r\n\r\n";
        let (_, req) = parse(raw);
        assert_eq!(req.body, b"Wikipedia");
    }

    #[test]
    fn head_reader_stops_one_byte_short_of_terminator() {
        let raw = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nREST";
        let mut r: &[u8] = raw;
        let head = read_head_raw(&mut r).unwrap().unwrap();
        // Head ends with the 3-byte terminator prefix; the final '\n' is pending.
        assert_eq!(head, b"GET / HTTP/1.1\r\nHost: x\r\n\r");
        let mut pending = Vec::new();
        r.read_to_end(&mut pending).unwrap();
        assert_eq!(pending, b"\nREST");
    }

    #[test]
    fn empty_stream_yields_eof() {
        let mut r: &[u8] = b"";
        assert!(matches!(read_head_raw(&mut r), Ok(None)));
    }

    #[test]
    fn malformed_request_line_is_invalid_data() {
        let head = read_head_raw(&mut &b"GET /\r\n\r\n"[..]).unwrap().unwrap();
        match parse_head(&head) {
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
            Ok(_) => panic!("expected malformed request line"),
        }
    }

    #[test]
    fn truncated_body_reports_abort_with_received_bytes() {
        let mut r: &[u8] = b"PUT /a HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc";
        let head = read_head_raw(&mut r).unwrap().unwrap();
        let mut req = parse_head(&head).unwrap();
        let mut reserved = [0u8; 1];
        r.read_exact(&mut reserved).unwrap();
        match read_body_into(&mut r, &mut req, BodyPlan::Full) {
            Err(BodyError::Aborted(n)) => assert_eq!(n, 3),
            other => panic!("expected aborted body, got {other:?}"),
        }
    }

    #[test]
    fn response_writes_status_headers_and_body() {
        let resp = Resp {
            status: 201,
            extra_headers: vec![("X-A", "b".to_owned())],
            content_length: 3,
            body: b"abc".to_vec(),
            drip: None,
        };
        let mut out = Vec::new();
        write_response(&mut out, &resp).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 201 Created\r\n"), "{text}");
        assert!(text.contains("Content-Length: 3\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.contains("X-A: b\r\n"));
        assert!(text.ends_with("\r\n\r\nabc"));
    }
}
