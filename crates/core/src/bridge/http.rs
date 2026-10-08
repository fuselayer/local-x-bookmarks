//! A minimal HTTP/1.1 server, hand-rolled on purpose.
//!
//! ## Why not `hyper`
//!
//! `hyper` is already in the dependency tree, so this is not a "avoid a
//! dependency" decision. It is a "keep the attack surface legible" decision.
//! The whole endpoint surface is one `GET` and two `POST`s on loopback, and
//! the limits that matter — head size, body size, framing, timeouts — are
//! easier to audit as twenty explicit lines than as a builder configuration
//! spread across three crates.
//!
//! ## What it refuses, and why each refusal is deliberate
//!
//! - **A request that declares its length two ways** — both `Content-Length`
//!   and `Transfer-Encoding`. That ambiguity is the classic request-smuggling
//!   shape, and it is refused rather than resolved by a guess.
//! - **Chunked encoding is decoded, not refused.** An earlier version of this
//!   file rejected it outright on the theory that the only client is our own
//!   userscript. That was wrong: Node's `http.request` switches to chunked the
//!   moment `Content-Length` is omitted, browsers streaming a body do the same,
//!   and the result was a capture that failed with a 400 the script could not
//!   explain. The cap is enforced against the *decoded* total, so chunking buys
//!   a sender nothing.
//! - **Bodies over [`MAX_BODY_BYTES`].** With `Content-Length` the cap is
//!   checked before allocating, so an oversize request costs a rejected header
//!   rather than a reservation.
//! - **Heads over [`MAX_HEAD_BYTES`].** Same reasoning, applied to the part
//!   we read line by line.
//!
//! Every response is `Connection: close`. There is no keep-alive, no pipelining
//! and no reuse, which removes a whole category of state-confusion bug at the
//! cost of a TCP handshake per request — and the only client sends a handful
//! of requests per browsing session.

use std::io::{BufRead, Write};

/// Largest request head we will read, request line and headers together.
pub const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Largest body we will accept.
///
/// A full bookmarks page is a few hundred kilobytes. Eight megabytes leaves
/// room for a page of long-form posts with media metadata while staying far
/// below anything that would trouble a desktop process.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Longest single header line. Guards the line-oriented read against a client
/// that never sends a newline.
const MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The raw request target, e.g. `/v1/hello?probe=abc`.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The path portion of the target, without the query string.
    ///
    /// Trailing slashes are trimmed so `/v1/pair` and `/v1/pair/` route the
    /// same way. A script that gets this wrong should not see a 404.
    pub fn path(&self) -> &str {
        let path = self.target.split(['?', '#']).next().unwrap_or(&self.target);
        if path.len() > 1 {
            path.trim_end_matches('/')
        } else {
            path
        }
    }

    /// The query string, without the leading `?`. Empty when there is none.
    pub fn query(&self) -> &str {
        match self.target.split_once('?') {
            Some((_, q)) => q.split('#').next().unwrap_or(q),
            None => "",
        }
    }

    /// Case-insensitive header lookup, as HTTP requires.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The body as text, or `None` if it is not valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }
}

#[derive(Debug)]
pub enum ReadError {
    /// The client closed before sending anything. Not an error worth logging.
    Eof,
    Malformed(String),
    /// Framing we refuse to guess at. Answered with 400, never with a guess.
    UnsupportedFraming(String),
    TooLarge(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Eof => write!(f, "client closed without sending a request"),
            ReadError::Malformed(m) => write!(f, "malformed request: {m}"),
            ReadError::UnsupportedFraming(m) => write!(f, "unsupported framing: {m}"),
            ReadError::TooLarge(m) => write!(f, "request too large: {m}"),
            ReadError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Read one request. `Ok(None)` means the peer connected and closed without
/// sending anything, which happens constantly with port probes and is not an
/// error condition.
pub fn read_request<R: BufRead>(reader: &mut R) -> Result<Option<Request>, ReadError> {
    read_request_with(reader, &mut |_| Ok(()))
}

/// As [`read_request`], with a hook that runs once the headers are parsed and
/// **before** the body is read.
///
/// This exists for `Expect: 100-continue`. A client that sends that header is
/// asking permission before transmitting a body, and a server that ignores it
/// leaves the client waiting out its own timeout. .NET's HTTP stack sends it by
/// default for POSTs, so without this hook a perfectly ordinary client stalls
/// for reasons that look like a hang rather than a protocol omission.
pub fn read_request_with<R: BufRead>(
    reader: &mut R,
    on_head: &mut dyn FnMut(&[(String, String)]) -> std::io::Result<()>,
) -> Result<Option<Request>, ReadError> {
    let mut budget = MAX_HEAD_BYTES;

    // ---- request line ----
    let Some(line) = read_line_bounded(reader, &mut budget)? else {
        return Ok(None);
    };
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        // Tolerate a stray leading CRLF, which some clients emit.
        let Some(line) = read_line_bounded(reader, &mut budget)? else {
            return Ok(None);
        };
        return parse_from(reader, line.trim_end_matches(['\r', '\n']).to_string(), budget, on_head);
    }
    parse_from(reader, line.to_string(), budget, on_head)
}

fn parse_from<R: BufRead>(
    reader: &mut R,
    request_line: String,
    mut budget: usize,
    on_head: &mut dyn FnMut(&[(String, String)]) -> std::io::Result<()>,
) -> Result<Option<Request>, ReadError> {
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let version = parts.next().unwrap_or_default().to_string();

    if method.is_empty() || target.is_empty() {
        return Err(ReadError::Malformed(format!("bad request line: {request_line:?}")));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(ReadError::UnsupportedFraming(format!("version {version:?}")));
    }

    // ---- headers ----
    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let Some(raw) = read_line_bounded(reader, &mut budget)? else {
            return Err(ReadError::Malformed("headers ended without a blank line".into()));
        };
        let raw = raw.trim_end_matches(['\r', '\n']);
        if raw.is_empty() {
            break;
        }
        let Some((name, value)) = raw.split_once(':') else {
            return Err(ReadError::Malformed(format!("header without a colon: {raw:?}")));
        };
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }

    // ---- framing ----
    //
    // The head is complete, so the caller gets its chance to answer
    // `Expect: 100-continue` before we block waiting for a body that the client
    // is holding back until it hears from us.
    on_head(&headers).map_err(ReadError::Io)?;

    let transfer_encoding = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding"))
        .map(|(_, v)| v.as_str());

    let lengths: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.as_str())
        .collect();

    if lengths.len() > 1 {
        return Err(ReadError::UnsupportedFraming("duplicate Content-Length".into()));
    }

    // The smuggling shape is a request that declares its length *two* ways, so
    // the recipient has to guess which one the sender meant. That is refused.
    //
    // Chunked encoding on its own is not that. It is simply what a client sends
    // when it does not know the length up front, and every mainstream HTTP
    // client will do it — Node's `http.request` switches to chunked the moment
    // you omit Content-Length, and a browser streaming a body does the same.
    // Refusing it outright would mean a capture silently 400s for a reason no
    // one could see from the script side.
    if transfer_encoding.is_some() && !lengths.is_empty() {
        return Err(ReadError::UnsupportedFraming(
            "both Content-Length and Transfer-Encoding present".into(),
        ));
    }

    let body = match transfer_encoding {
        Some(value) => {
            let value = value.trim();
            // `chunked` is the only transfer coding defined for HTTP/1.1
            // requests in practice. Anything else is refused rather than
            // guessed at.
            if !value.eq_ignore_ascii_case("chunked") {
                return Err(ReadError::UnsupportedFraming(format!(
                    "Transfer-Encoding: {value}"
                )));
            }
            read_chunked(reader)?
        }
        None => {
            let len: usize = match lengths.first() {
                Some(v) => v
                    .parse()
                    .map_err(|_| ReadError::Malformed(format!("non-numeric Content-Length: {v:?}")))?,
                None => 0,
            };

            if len > MAX_BODY_BYTES {
                return Err(ReadError::TooLarge(format!(
                    "Content-Length {len} exceeds the {MAX_BODY_BYTES} byte cap"
                )));
            }

            let mut body = vec![0u8; len];
            if len > 0 {
                reader.read_exact(&mut body)?;
            }
            body
        }
    };

    Ok(Some(Request { method, target, headers, body }))
}

/// Decode a `Transfer-Encoding: chunked` body.
///
/// The cap is enforced against the *decoded* total, so a stream of small chunks
/// cannot be used to grow the buffer past [`MAX_BODY_BYTES`] any more than a
/// large `Content-Length` could.
fn read_chunked<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, ReadError> {
    let mut out: Vec<u8> = Vec::new();
    let mut budget = MAX_HEAD_BYTES;

    loop {
        let Some(line) = read_line_bounded(reader, &mut budget)? else {
            return Err(ReadError::Malformed("chunked body ended mid-chunk".into()));
        };
        let line = line.trim_end_matches(['\r', '\n']);

        // Chunk extensions (";name=value") carry no meaning for us and are
        // legal, so they are stripped rather than rejected.
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| ReadError::Malformed(format!("bad chunk size {size_text:?}")))?;

        if size == 0 {
            // Consume any trailer headers up to the terminating blank line.
            loop {
                match read_line_bounded(reader, &mut budget)? {
                    Some(t) if t.trim_end_matches(['\r', '\n']).is_empty() => break,
                    Some(_) => continue,
                    None => break,
                }
            }
            return Ok(out);
        }

        if out.len() + size > MAX_BODY_BYTES {
            return Err(ReadError::TooLarge(format!(
                "chunked body exceeds the {MAX_BODY_BYTES} byte cap"
            )));
        }

        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk)?;
        out.extend_from_slice(&chunk);

        // Each chunk is followed by its own CRLF. A chunk that is not is a
        // framing error, not something to resynchronise on.
        match read_line_bounded(reader, &mut budget)? {
            Some(t) if t.trim_end_matches(['\r', '\n']).is_empty() => {}
            _ => return Err(ReadError::Malformed("chunk not terminated by CRLF".into())),
        }
    }
}

/// Read one line, consuming from a shared head budget. `Ok(None)` on clean EOF
/// at a line boundary.
fn read_line_bounded<R: BufRead>(
    reader: &mut R,
    budget: &mut usize,
) -> Result<Option<String>, ReadError> {
    let mut buf = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let n = reader.read(&mut byte)?;
        if n == 0 {
            if buf.is_empty() {
                return Ok(None);
            }
            break;
        }
        if *budget == 0 {
            return Err(ReadError::TooLarge("request head exceeded the cap".into()));
        }
        *budget -= 1;
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() > MAX_LINE_BYTES {
            return Err(ReadError::TooLarge("header line exceeded the cap".into()));
        }
    }
    // Headers are ASCII by spec. Anything else is a client we do not serve,
    // and lossy conversion keeps this from being a panic path.
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

/// Write a complete response and close the connection.
pub fn write_response(
    out: &mut impl Write,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = reason_phrase(status);
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    out.write_all(head.as_bytes())?;
    out.write_all(body)?;
    out.flush()
}

/// Write a JSON response.
pub fn write_json(out: &mut impl Write, status: u16, value: &serde_json::Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    write_response(out, status, "application/json; charset=utf-8", &body)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        413 => "Content Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    fn parse(raw: &str) -> Result<Option<Request>, ReadError> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn reads_a_well_formed_post() {
        let req = parse("POST /v1/capture HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 5\r\n\r\nhello")
            .unwrap()
            .unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path(), "/v1/capture");
        assert_eq!(req.text(), Some("hello"));
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        // HTTP says header names are case-insensitive, and clients vary.
        let req = parse("GET / HTTP/1.1\r\nAUTHORIZATION: Bearer abc\r\n\r\n").unwrap().unwrap();
        assert_eq!(req.header("authorization"), Some("Bearer abc"));
        assert_eq!(req.header("Authorization"), Some("Bearer abc"));
    }

    #[test]
    fn splits_the_query_string_off_the_path() {
        let req = parse("GET /v1/hello?probe=deadbeef HTTP/1.1\r\n\r\n").unwrap().unwrap();
        assert_eq!(req.path(), "/v1/hello");
        assert_eq!(req.query(), "probe=deadbeef");
    }

    #[test]
    fn tolerates_a_trailing_slash() {
        let req = parse("POST /v1/pair/ HTTP/1.1\r\nContent-Length: 0\r\n\r\n").unwrap().unwrap();
        assert_eq!(req.path(), "/v1/pair");
    }

    #[test]
    fn a_body_is_read_exactly_and_not_one_byte_more() {
        // The stream continues past the declared body; the parser must stop at
        // Content-Length rather than draining the socket.
        let raw = "POST /x HTTP/1.1\r\nContent-Length: 3\r\n\r\nabcTRAILING";
        let mut cursor = Cursor::new(raw.as_bytes().to_vec());
        let req = read_request(&mut cursor).unwrap().unwrap();
        assert_eq!(req.body, b"abc");
        let mut rest = Vec::new();
        cursor.read_to_end(&mut rest).unwrap();
        assert_eq!(rest, b"TRAILING");
    }

    #[test]
    fn an_empty_body_needs_no_content_length() {
        let req = parse("GET /v1/hello HTTP/1.1\r\n\r\n").unwrap().unwrap();
        assert!(req.body.is_empty());
    }

    #[test]
    fn decodes_a_chunked_body() {
        // Not a nicety: Node's http.request uses chunked whenever
        // Content-Length is omitted, and browsers streaming a body do too.
        // Refusing it made captures fail with an inexplicable 400.
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                   5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let req = parse(raw).unwrap().unwrap();
        assert_eq!(req.text(), Some("hello world"));
    }

    #[test]
    fn decodes_a_chunked_body_with_extensions_and_trailers() {
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n\
                   5;name=value\r\nhello\r\n0\r\nX-Checksum: abc\r\n\r\n";
        let req = parse(raw).unwrap().unwrap();
        assert_eq!(req.text(), Some("hello"));
    }

    #[test]
    fn decodes_an_empty_chunked_body() {
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let req = parse(raw).unwrap().unwrap();
        assert!(req.body.is_empty());
    }

    #[test]
    fn refuses_a_request_that_declares_its_length_both_ways() {
        // This is the actual smuggling shape: two framings, so the recipient
        // has to guess which one the sender meant.
        let raw = "POST /x HTTP/1.1\r\nContent-Length: 5\r\n\
                   Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
        let err = parse(raw).unwrap_err();
        assert!(matches!(err, ReadError::UnsupportedFraming(_)), "got {err:?}");
    }

    #[test]
    fn refuses_a_transfer_coding_it_does_not_implement() {
        let err = parse("POST /x HTTP/1.1\r\nTransfer-Encoding: gzip\r\n\r\n").unwrap_err();
        assert!(matches!(err, ReadError::UnsupportedFraming(_)), "got {err:?}");
    }

    #[test]
    fn a_chunked_body_cannot_exceed_the_cap_by_being_split_up() {
        // The cap applies to the decoded total, so many small chunks are no
        // more able to grow the buffer than one large Content-Length.
        let mut raw = String::from("POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n");
        let chunk = "a".repeat(64 * 1024);
        for _ in 0..(MAX_BODY_BYTES / chunk.len() + 2) {
            raw.push_str(&format!("{:x}\r\n{}\r\n", chunk.len(), chunk));
        }
        raw.push_str("0\r\n\r\n");
        let err = parse(&raw).unwrap_err();
        assert!(matches!(err, ReadError::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn a_truncated_chunked_body_is_an_error_not_a_short_read() {
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";
        let err = parse(raw).unwrap_err();
        assert!(matches!(err, ReadError::Io(_)), "got {err:?}");
    }

    #[test]
    fn a_chunk_with_a_bogus_size_is_refused() {
        let raw = "POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nhello\r\n0\r\n\r\n";
        let err = parse(raw).unwrap_err();
        assert!(matches!(err, ReadError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn refuses_duplicate_content_length() {
        // Ambiguous framing is rejected, not resolved by picking one.
        let err = parse("POST /x HTTP/1.1\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd")
            .unwrap_err();
        assert!(matches!(err, ReadError::UnsupportedFraming(_)), "got {err:?}");
    }

    #[test]
    fn refuses_a_non_numeric_content_length() {
        let err = parse("POST /x HTTP/1.1\r\nContent-Length: three\r\n\r\n").unwrap_err();
        assert!(matches!(err, ReadError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn refuses_an_oversize_body_before_allocating_it() {
        // The declared length is checked against the cap up front, so a hostile
        // Content-Length costs a rejected header, not a reservation.
        let raw = format!("POST /x HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        let err = parse(&raw).unwrap_err();
        assert!(matches!(err, ReadError::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn refuses_an_endless_header_line() {
        let raw = format!("GET / HTTP/1.1\r\nX-Big: {}\r\n\r\n", "a".repeat(MAX_LINE_BYTES + 10));
        let err = parse(&raw).unwrap_err();
        assert!(matches!(err, ReadError::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn refuses_an_endless_run_of_headers() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        for i in 0..4000 {
            raw.push_str(&format!("X-{i}: {}\r\n", "v".repeat(20)));
        }
        raw.push_str("\r\n");
        let err = parse(&raw).unwrap_err();
        assert!(matches!(err, ReadError::TooLarge(_)), "got {err:?}");
    }

    #[test]
    fn a_silent_connection_is_eof_not_an_error() {
        // Port probes do this constantly.
        assert!(parse("").unwrap().is_none());
    }

    #[test]
    fn a_truncated_body_is_an_io_error_not_a_short_read() {
        // Claiming 10 bytes and sending 3 must not yield a 3-byte body.
        let err = parse("POST /x HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc").unwrap_err();
        assert!(matches!(err, ReadError::Io(_)), "got {err:?}");
    }

    #[test]
    fn refuses_a_header_without_a_colon() {
        let err = parse("GET / HTTP/1.1\r\nnonsense\r\n\r\n").unwrap_err();
        assert!(matches!(err, ReadError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn refuses_a_non_http_version() {
        let err = parse("GET / HTTP/2.0\r\n\r\n").unwrap_err();
        assert!(matches!(err, ReadError::UnsupportedFraming(_)), "got {err:?}");
    }

    #[test]
    fn responses_always_carry_a_length_and_close() {
        let mut out = Vec::new();
        write_response(&mut out, 200, "application/json", b"{}").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }

    #[test]
    fn a_non_utf8_body_is_none_rather_than_a_panic() {
        let raw = b"POST /x HTTP/1.1\r\nContent-Length: 2\r\n\r\n\xff\xfe".to_vec();
        let req = read_request(&mut Cursor::new(raw)).unwrap().unwrap();
        assert!(req.text().is_none());
    }
}
