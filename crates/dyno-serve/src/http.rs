//! A small HTTP/1.1 server side: one request per connection, and the connection closes after the
//! response.
//!
//! The server speaks plain HTTP only. Safari tries HTTPS on a port before it falls back to HTTP,
//! so a connection can start with a TLS client hello. The server refuses that at once, with a
//! TLS alert, so the browser falls back without waiting for an answer that never comes.

use std::sync::Arc;
use std::time::Duration;

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

/// The first byte of a TLS connection: the type of a handshake record, which holds the client
/// hello. An HTTP request starts with its method, which is letters, so one byte tells them apart.
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// The server's answer to a TLS client hello: one alert record (type 0x15, TLS 1.2, 2 bytes) with
/// a fatal (2) `handshake_failure` (40). A TLS client reads it as "no TLS here", and stops.
const TLS_REFUSAL: [u8; 7] = [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28];

/// How long the server reads what a refused TLS client still sends, before it closes.
const TLS_REFUSAL_DRAIN: Duration = Duration::from_secs(1);

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
pub struct Response<'b> {
    pub status: &'static str,
    pub content_type: &'static str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Body<'b>,
}

/// A response's body. A build is held in memory once, and a response for one of its files is
/// written from there: the response holds no copy of the file, and a HEAD request reads only
/// its length.
#[derive(Debug)]
pub enum Body<'b> {
    /// Bytes that the response made, such as an error's words or a command.
    Owned(Vec<u8>),
    /// Bytes that something else holds, such as a build file or a part of one.
    Shared(&'b [u8]),
    /// An HTML page of the build with a tag put in at `at`. The page is written from where it
    /// is, in two parts, with the tag between them.
    Tagged {
        page: &'b [u8],
        tag: &'static [u8],
        at: usize,
    },
}

impl Body<'_> {
    /// How many bytes the body has. A tagged page has its own and the tag's, wherever the tag
    /// goes, so a HEAD request reads no more than the two lengths.
    pub fn len(&self) -> usize {
        match self {
            Body::Owned(bytes) => bytes.len(),
            Body::Shared(bytes) => bytes.len(),
            Body::Tagged { page, tag, .. } => page.len() + tag.len(),
        }
    }

    /// The body's bytes, in the order they are written. A part can be empty.
    fn parts(&self) -> [&[u8]; 3] {
        match self {
            Body::Owned(bytes) => [bytes, &[], &[]],
            Body::Shared(bytes) => [bytes, &[], &[]],
            Body::Tagged { page, tag, at } => [&page[..*at], tag, &page[*at..]],
        }
    }

    /// The body in one piece, for a test to read.
    #[cfg(test)]
    pub fn to_vec(&self) -> Vec<u8> {
        self.parts().concat()
    }
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
/// HTTP. A TLS client hello is refused as soon as its first byte comes.
pub async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buffer = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let head_length = loop {
        if buffer.first() == Some(&TLS_HANDSHAKE_RECORD) {
            refuse_tls(socket).await;
            return None;
        }
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

/// Tells a TLS client that this port has no TLS, and closes. A client hello has no end of a
/// request head, so without this the server would wait for more while the client waits for the
/// server's hello, until the client gives up.
///
/// The server reads on for a moment after its answer. A socket closed with bytes still unread
/// resets the connection, and a reset can reach the client before the alert does.
async fn refuse_tls(socket: &mut TcpStream) {
    if socket.write_all(&TLS_REFUSAL).await.is_err() {
        return;
    }
    let _ = socket.shutdown().await;
    let mut rest = [0_u8; 4096];
    let drained = async { while matches!(socket.read(&mut rest).await, Ok(count) if count > 0) {} };
    let _ = tokio::time::timeout(TLS_REFUSAL_DRAIN, drained).await;
}

/// Writes the response, without its body for a HEAD request, and closes the connection. Returns
/// the body bytes sent.
pub async fn write_response(
    socket: &mut TcpStream,
    response: &Response<'_>,
    head_only: bool,
) -> u64 {
    let length = response.body.len();
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\n\
         Connection: close\r\n",
        response.status, response.content_type,
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
    if !head_only {
        sent = length as u64;
        for part in response.body.parts() {
            if socket.write_all(part).await.is_err() {
                sent = 0;
                break;
            }
        }
    }
    let _ = socket.shutdown().await;
    sent
}

impl<'b> Response<'b> {
    /// A response with no extra headers, whose body it made.
    pub fn new(
        status: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> Response<'static> {
        Response {
            status,
            content_type,
            headers: Vec::new(),
            body: Body::Owned(body),
        }
    }

    /// A response with no extra headers, whose body something else holds.
    pub fn shared(
        status: &'static str,
        content_type: &'static str,
        body: &'b [u8],
    ) -> Response<'b> {
        Response {
            status,
            content_type,
            headers: Vec::new(),
            body: Body::Shared(body),
        }
    }

    /// A plain-text error response.
    pub fn error(status: &'static str) -> Response<'static> {
        Response::shared(status, "text/plain; charset=utf-8", status.as_bytes())
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

    /// The start of a TLS 1.3 client hello, as a browser sends it: a handshake record (0x16) of
    /// 512 bytes that holds a client hello (0x01). The rest of the record never comes.
    const CLIENT_HELLO_START: [u8; 11] = [
        0x16, 0x03, 0x01, 0x02, 0x00, 0x01, 0x00, 0x01, 0xfc, 0x03, 0x03,
    ];

    #[tokio::test(flavor = "current_thread")]
    async fn a_tls_client_hello_is_refused_at_once_with_an_alert() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await
        });
        // The client sends the start of its hello and waits for the server's, as a browser does:
        // it does not close, and it sends no end of a request head.
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&CLIENT_HELLO_START).await.unwrap();
        let mut answer = Vec::new();
        let answered =
            tokio::time::timeout(Duration::from_millis(500), client.read_to_end(&mut answer)).await;
        assert!(answered.is_ok(), "the server did not answer and close");
        assert_eq!(answer, TLS_REFUSAL);
        // The alert is fatal, and says the handshake failed.
        assert_eq!(answer[5..], [2, 40]);
        drop(client);
        assert!(server.await.unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_tls_client_hello_is_known_by_its_first_byte_alone() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await
        });
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&CLIENT_HELLO_START[..1]).await.unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_millis(500), client.read_to_end(&mut answer))
            .await
            .expect("the server did not answer and close")
            .unwrap();
        assert_eq!(answer, TLS_REFUSAL);
        drop(client);
        assert!(server.await.unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_refused_tls_client_that_stays_open_does_not_hold_the_server() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(address).await.unwrap();
        client.write_all(&CLIENT_HELLO_START).await.unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        // The client never closes, so the server stops reading by itself.
        let refused = tokio::time::timeout(
            TLS_REFUSAL_DRAIN + Duration::from_secs(1),
            read_request(&mut socket),
        )
        .await;
        assert!(matches!(refused, Ok(None)), "{refused:?}");
        drop(client);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_head_too_large_is_not_read() {
        static LONG: [u8; MAX_HEAD_BYTES + 10] = [b'a'; MAX_HEAD_BYTES + 10];
        assert!(read_from(&LONG).await.is_none());
    }

    /// What a client gets when the server writes this response to it.
    async fn written(response: &Response<'_>, head_only: bool) -> (u64, String) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = tokio::spawn(async move {
            let mut socket = TcpStream::connect(address).await.unwrap();
            let mut got = String::new();
            socket.read_to_string(&mut got).await.unwrap();
            got
        });
        let (mut socket, _) = listener.accept().await.unwrap();
        let sent = write_response(&mut socket, response, head_only).await;
        drop(socket);
        (sent, client.await.unwrap())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_shared_body_is_written_from_where_it_is() {
        let held = b"0123456789".to_vec();
        let response = Response::shared("200 OK", "text/plain", &held[2..5]);
        // The response points into the bytes that are held: it has no copy of them.
        let Body::Shared(body) = response.body else {
            panic!("{response:?}");
        };
        assert!(std::ptr::eq(body, &held[2..5]));
        let (sent, got) = written(&response, false).await;
        assert_eq!(sent, 3);
        assert!(got.contains("Content-Length: 3\r\n"), "{got}");
        assert!(got.ends_with("\r\n\r\n234"), "{got}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_tagged_page_is_written_in_its_parts_with_the_tag_between() {
        let response = Response {
            status: "200 OK",
            content_type: "text/html",
            headers: Vec::new(),
            body: Body::Tagged {
                page: b"<head></head>",
                tag: b"<script></script>",
                at: 6,
            },
        };
        assert_eq!(response.body.len(), 30);
        let (sent, got) = written(&response, false).await;
        assert_eq!(sent, 30);
        assert!(got.contains("Content-Length: 30\r\n"), "{got}");
        assert!(
            got.ends_with("\r\n\r\n<head><script></script></head>"),
            "{got}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_head_request_gets_the_length_and_no_body() {
        let held = vec![b'a'; 1_000];
        let response = Response::shared("200 OK", "text/plain", &held);
        let (sent, got) = written(&response, true).await;
        assert_eq!(sent, 0);
        assert!(got.contains("Content-Length: 1000\r\n"), "{got}");
        assert!(got.ends_with("Connection: close\r\n\r\n"), "{got}");
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
