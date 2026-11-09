//! A small HTTP/1.1 server side: one request per connection, and the connection closes after the
//! response.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The most of a request head the server reads.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// The most of a request body the server reads: a batch of probe events.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// The most headers a request may have.
const MAX_HEADERS: usize = 64;

/// The end of a request head.
const HEAD_END: &[u8] = b"\r\n\r\n";

/// One request, as the server needs it.
#[derive(Debug)]
pub struct Request {
    pub method: String,
    /// The path, without the query string or fragment, still percent-encoded.
    pub path: String,
    /// The `Range` header, as sent.
    pub range: Option<String>,
    pub body: Vec<u8>,
}

/// One response. Every response has `Cache-Control: no-store`, so each run loads afresh.
#[derive(Debug)]
pub struct Response {
    pub status: &'static str,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// What a `Range` header asks for, within a body of known length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeAsk {
    /// No range, or one the server does not read, such as several ranges: the whole body.
    Whole,
    /// The bytes from the first to the last, both included.
    Part(u64, u64),
    /// A range that starts past the end.
    Unsatisfiable,
}

/// Serves each connection on its own task, with what the server shares between them.
pub async fn accept<S, F, Fut>(listener: TcpListener, shared: Arc<S>, serve: F)
where
    F: Fn(Arc<S>, TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    while let Ok((socket, _)) = listener.accept().await {
        tokio::spawn(serve(Arc::clone(&shared), socket));
    }
}

/// Reads one request. None when the client hung up, or sent too much or something that is not
/// HTTP.
pub async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let head_length = loop {
        if let Some(end) = memchr::memmem::find(&buffer, HEAD_END) {
            break end + HEAD_END.len();
        }
        if buffer.len() >= MAX_HEAD_BYTES {
            return None;
        }
        let count = socket.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..count]);
    };
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut headers);
    let httparse::Status::Complete(_) = parsed.parse(&buffer[..head_length]).ok()? else {
        return None;
    };
    let method = parsed.method?.to_owned();
    let target = parsed.path?;
    let path = target.split(['?', '#']).next().unwrap_or(target).to_owned();
    let header = |name: &str| {
        parsed
            .headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .and_then(|header| std::str::from_utf8(header.value).ok())
            .map(|value| value.trim().to_owned())
    };
    let length = match header("content-length") {
        Some(value) => value.parse::<usize>().ok()?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return None;
    }
    let range = header("range");
    let mut body = buffer[head_length..].to_vec();
    while body.len() < length {
        let count = socket.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..count]);
    }
    body.truncate(length);
    Some(Request {
        method,
        path,
        range,
        body,
    })
}

/// Writes the response, without its body for a HEAD request, and closes the connection. Returns
/// the body bytes sent.
pub async fn write_response(socket: &mut TcpStream, response: &Response, head_only: bool) -> u64 {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Connection: close\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    // A client that hangs up early gets nothing more.
    if socket.write_all(head.as_bytes()).await.is_err() {
        return 0;
    }
    let mut sent = 0;
    if !head_only && socket.write_all(&response.body).await.is_ok() {
        sent = response.body.len() as u64;
    }
    let _ = socket.shutdown().await;
    sent
}

impl Response {
    /// A response with no extra headers.
    pub fn new(status: &'static str, content_type: &'static str, body: Vec<u8>) -> Response {
        Response {
            status,
            content_type,
            headers: Vec::new(),
            body,
        }
    }

    /// A plain-text error response.
    pub fn error(status: &'static str) -> Response {
        Response::new(
            status,
            "text/plain; charset=utf-8",
            status.as_bytes().to_vec(),
        )
    }
}

/// What a `Range` header asks for: `bytes=first-last`, `bytes=first-` or `bytes=-suffix`, in a
/// body of `length` bytes. Safari needs ranges to play an `<audio>` or `<video>` file.
pub fn range_ask(header: Option<&str>, length: u64) -> RangeAsk {
    let Some(spec) = header.and_then(|header| header.strip_prefix("bytes=")) else {
        return RangeAsk::Whole;
    };
    if spec.contains(',') {
        return RangeAsk::Whole;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeAsk::Whole;
    };
    let parsed = (first.parse::<u64>().ok(), last.parse::<u64>().ok());
    let (start, end) = match parsed {
        (Some(start), Some(end)) if start <= end => (start, end.min(length.saturating_sub(1))),
        (Some(start), None) if last.is_empty() => (start, length.saturating_sub(1)),
        (None, Some(suffix)) if first.is_empty() && suffix > 0 => {
            (length.saturating_sub(suffix), length.saturating_sub(1))
        }
        _ => return RangeAsk::Whole,
    };
    if start >= length {
        return RangeAsk::Unsatisfiable;
    }
    RangeAsk::Part(start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `read_request` makes of a client that sends these bytes, then stops sending.
    async fn read_from(bytes: &'static [u8]) -> Option<Request> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut socket = TcpStream::connect(address).await.unwrap();
            socket.write_all(bytes).await.unwrap();
            socket.shutdown().await.unwrap();
            // Held open until the server has read, so the server sees the end of the bytes.
            let mut rest = Vec::new();
            let _ = socket.read_to_end(&mut rest).await;
        });
        let (mut socket, _) = listener.accept().await.unwrap();
        let request = read_request(&mut socket).await;
        drop(socket);
        client.await.unwrap();
        request
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_request_with_a_body_is_read_whole() {
        let request = read_from(b"POST /a?b=1 HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello")
            .await
            .unwrap();
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/a")
        );
        assert_eq!(request.body, b"hello");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_request_that_is_not_http_or_ends_early_is_not_read() {
        let cases: [&'static [u8]; 5] = [
            b"NOT HTTP AT ALL\r\n\r\n",
            b"GET / HTTP/1.1\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc",
            b"POST / HTTP/1.1\r\nContent-Length: ten\r\n\r\n",
            b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n",
        ];
        for bytes in cases {
            assert!(
                read_from(bytes).await.is_none(),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_head_too_large_is_not_read() {
        static LONG: [u8; MAX_HEAD_BYTES + 10] = [b'a'; MAX_HEAD_BYTES + 10];
        assert!(read_from(&LONG).await.is_none());
    }

    #[test]
    fn no_range_or_another_unit_asks_for_the_whole_body() {
        assert_eq!(range_ask(None, 100), RangeAsk::Whole);
        assert_eq!(range_ask(Some("items=0-1"), 100), RangeAsk::Whole);
    }

    #[test]
    fn a_closed_range_is_cut_to_the_body() {
        assert_eq!(range_ask(Some("bytes=0-1"), 100), RangeAsk::Part(0, 1));
        assert_eq!(range_ask(Some("bytes=90-200"), 100), RangeAsk::Part(90, 99));
    }

    #[test]
    fn an_open_range_runs_to_the_end() {
        assert_eq!(range_ask(Some("bytes=10-"), 100), RangeAsk::Part(10, 99));
    }

    #[test]
    fn a_suffix_range_is_the_last_bytes() {
        assert_eq!(range_ask(Some("bytes=-10"), 100), RangeAsk::Part(90, 99));
        assert_eq!(range_ask(Some("bytes=-500"), 100), RangeAsk::Part(0, 99));
    }

    #[test]
    fn a_range_past_the_end_cannot_be_met() {
        assert_eq!(range_ask(Some("bytes=100-"), 100), RangeAsk::Unsatisfiable);
        assert_eq!(range_ask(Some("bytes=0-0"), 0), RangeAsk::Unsatisfiable);
    }

    #[test]
    fn several_ranges_or_a_bad_one_ask_for_the_whole_body() {
        assert_eq!(range_ask(Some("bytes=0-1,5-6"), 100), RangeAsk::Whole);
        assert_eq!(range_ask(Some("bytes=5-1"), 100), RangeAsk::Whole);
        assert_eq!(range_ask(Some("bytes=-0"), 100), RangeAsk::Whole);
        assert_eq!(range_ask(Some("bytes=x-"), 100), RangeAsk::Whole);
    }
}
