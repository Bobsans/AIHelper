//! One HTTP mock server for the plugins that talk to an HTTP API.
//!
//! There were three hand-written copies of this, one per plugin, and they had
//! drifted: only two put the accepted socket back into blocking mode, only one
//! bounded the accept loop, one carried its response body as a `String` while
//! the others used bytes, and the three status reason tables listed different
//! codes. The missing blocking-mode line was a real flake that somebody had
//! already fixed twice without a way to carry the fix to the third copy.
//!
//! So this is deliberately the *union*: every guard any copy had, and the byte
//! body, because a log archive is not text.
//!
//! A dev-dependency only. Nothing here ships in a plugin.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How long the server waits for the request it was told to expect.
///
/// Generous on purpose. It bounds a genuinely stuck test; it is not a latency
/// assertion, and a few seconds is not enough when the whole workspace is
/// building and testing in parallel.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long `requests` waits for the server thread to finish before answering
/// with what it has.
///
/// Short on purpose, unlike the accept timeout: by the time a test asks, the
/// client has already read the last response, so the thread is a few
/// microseconds from returning. The wait only ever elapses when a queued
/// response was never asked for, and then the answer is complete anyway.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// A request the server received, as the test sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    /// Header names lowercased, because HTTP header names are case-insensitive
    /// and a test should not have to guess how a client spelled one.
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl CapturedRequest {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// One response, queued in the order the test expects the requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockResponse {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl MockResponse {
    #[must_use]
    pub fn json(status: u16, body: &str) -> Self {
        Self::bytes(status, "application/json", body.as_bytes().to_vec())
    }

    #[must_use]
    pub fn text(status: u16, body: &str) -> Self {
        Self::bytes(status, "text/plain", body.as_bytes().to_vec())
    }

    #[must_use]
    pub fn empty(status: u16) -> Self {
        Self::bytes(status, "application/json", Vec::new())
    }

    #[must_use]
    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.to_owned(),
            body,
        }
    }
}

/// A loopback HTTP server that answers a fixed list of responses, once each,
/// and records what it was asked.
#[derive(Debug)]
pub struct MockServer {
    url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockServer {
    /// Bind a port and serve `responses`, one per request, in order.
    ///
    /// # Panics
    ///
    /// When the loopback port cannot be bound, which is a broken test
    /// environment rather than a failing assertion.
    #[must_use]
    pub fn new(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock server should bind");
        listener
            .set_nonblocking(true)
            .expect("listener should be nonblocking");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("local addr should exist")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + ACCEPT_TIMEOUT;
            for response in responses {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            handle_connection(stream, response, &captured);
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            // A test may queue a response the code under test
                            // never asks for. Without this the server would sit
                            // here until the accept timeout and `drop` would
                            // wait for it, turning a passing test into a
                            // minute-long one.
                            if stopped.load(Ordering::Relaxed) || Instant::now() > deadline {
                                return;
                            }
                            thread::sleep(POLL_INTERVAL);
                        }
                        Err(_) => return,
                    }
                }
            }
        });

        Self {
            url,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    #[must_use]
    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// What the server was asked, once it has finished answering.
    ///
    /// Waits for the server thread so that a test reading this does not race the
    /// last request, and gives up after [`DRAIN_TIMEOUT`] rather than hanging -
    /// a test that queued more responses than the code makes requests would
    /// otherwise wait for the accept timeout.
    #[must_use]
    pub fn requests(&self) -> Vec<CapturedRequest> {
        if let Some(handle) = &self.handle {
            let deadline = Instant::now() + DRAIN_TIMEOUT;
            while !handle.is_finished() && Instant::now() < deadline {
                thread::sleep(POLL_INTERVAL / 2);
            }
        }
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    response: MockResponse,
    requests: &Arc<Mutex<Vec<CapturedRequest>>>,
) {
    // On Windows an accepted socket inherits the listener's non-blocking mode,
    // so the first read races the client's request bytes; POSIX does not
    // inherit it. Two of the three copies of this file had the line and one did
    // not, which is what made the missing one flake on Windows alone.
    stream
        .set_nonblocking(false)
        .expect("accepted stream should be blocking");
    let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));
    let mut first_line = String::new();
    reader
        .read_line(&mut first_line)
        .expect("request line should read");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();

    let mut headers = HashMap::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).expect("header should read");
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            let key = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if key == "content-length" {
                content_length = value.parse::<usize>().unwrap_or(0);
            }
            headers.insert(key, value);
        }
    }

    let mut body_bytes = vec![0; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body_bytes)
            .expect("request body should read");
    }
    requests
        .lock()
        .expect("requests lock")
        .push(CapturedRequest {
            method,
            path,
            headers,
            body: String::from_utf8_lossy(&body_bytes).into_owned(),
        });

    let response_headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason_phrase(response.status),
        response.content_type,
        response.body.len()
    );
    stream
        .write_all(response_headers.as_bytes())
        .expect("response headers should write");
    stream
        .write_all(&response.body)
        .expect("response body should write");
}

/// The reason phrase for the statuses these tests use.
///
/// The three copies listed different subsets, so a 500 came back as `OK` in two
/// of them. Nothing asserts on the phrase today, but a mock that misreports the
/// status line is a bad place to start debugging.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufWriter;

    /// The response headers as text and the body as bytes, because a body is
    /// not necessarily text - which is the whole reason this server carries one
    /// as `Vec<u8>`.
    fn request(url: &str, path: &str, body: Option<&str>) -> (String, Vec<u8>) {
        let address = url.trim_start_matches("http://");
        let mut stream = TcpStream::connect(address).expect("the mock server should accept");
        let method = if body.is_some() { "POST" } else { "GET" };
        let payload = body.unwrap_or_default();
        let sent = format!(
            "{method} {path} HTTP/1.1\r\nHost: {address}\r\nX-Test: yes\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        );
        {
            let mut writer = BufWriter::new(&mut stream);
            writer.write_all(sent.as_bytes()).expect("write request");
        }
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).expect("read the response");
        let separator = answer
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("a complete response");
        let headers = String::from_utf8(answer[..separator].to_vec()).expect("headers are text");
        (headers, answer[separator + 4..].to_vec())
    }

    fn text_body(url: &str, path: &str, body: Option<&str>) -> (String, String) {
        let (headers, body) = request(url, path, body);
        (headers, String::from_utf8(body).expect("a text body"))
    }

    #[test]
    fn the_queued_responses_are_served_in_order_and_the_requests_recorded() {
        let server = MockServer::new(vec![
            MockResponse::json(200, r#"{"first":true}"#),
            MockResponse::text(500, "second"),
        ]);

        let (headers, body) = text_body(&server.url(), "/one", None);
        assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
        assert!(
            headers.contains("Content-Type: application/json"),
            "{headers}"
        );
        assert_eq!(body, r#"{"first":true}"#);

        let (headers, body) = text_body(&server.url(), "/two", Some("payload"));
        assert!(
            headers.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
            "the status line must report the status it was given: {headers}"
        );
        assert!(headers.contains("Content-Type: text/plain"), "{headers}");
        assert_eq!(body, "second");

        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/one");
        assert_eq!(requests[0].body, "");
        assert_eq!(requests[0].header("x-test"), Some("yes"));
        assert_eq!(
            requests[0].header("X-TEST"),
            Some("yes"),
            "a header is looked up case-insensitively"
        );
        assert_eq!(requests[1].method, "POST");
        assert_eq!(requests[1].body, "payload");
    }

    #[test]
    fn an_empty_response_still_carries_its_status() {
        let server = MockServer::new(vec![MockResponse::empty(204)]);
        let (headers, body) = text_body(&server.url(), "/gone", None);
        assert!(
            headers.starts_with("HTTP/1.1 204 No Content\r\n"),
            "{headers}"
        );
        assert!(headers.contains("Content-Length: 0"), "{headers}");
        assert!(body.is_empty());
    }

    /// A body that is not text: the archive case, which one of the three copies
    /// could not express.
    #[test]
    fn a_response_body_can_be_arbitrary_bytes() {
        let payload = vec![0x50, 0x4b, 0x03, 0x04, 0x00, 0xff];
        let server = MockServer::new(vec![MockResponse::bytes(
            200,
            "application/zip",
            payload.clone(),
        )]);
        let (headers, received) = request(&server.url(), "/logs", None);
        assert_eq!(received, payload, "the bytes arrive unchanged");
        assert!(
            headers.contains("Content-Type: application/zip"),
            "{headers}"
        );
        assert!(
            headers.contains(&format!("Content-Length: {}", payload.len())),
            "{headers}"
        );
    }

    /// Reading the requests must not hang when the code under test made fewer
    /// requests than the test queued responses for.
    #[test]
    fn requests_answers_even_when_a_queued_response_is_never_asked_for() {
        let server = MockServer::new(vec![
            MockResponse::json(200, "{}"),
            MockResponse::empty(404),
        ]);
        let _ = request(&server.url(), "/only-one", None);

        let started = Instant::now();
        let requests = server.requests();

        assert_eq!(requests.len(), 1);
        assert!(
            started.elapsed() < ACCEPT_TIMEOUT,
            "reading the requests waited for the accept timeout"
        );
    }
}
