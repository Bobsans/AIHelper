use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use super::common::IsolatedAhCommand as Command;
use predicates::{prelude::PredicateBooleanExt, str::contains};
use tempfile::TempDir;

#[derive(Clone)]
struct MockResponse {
    expected_method: &'static str,
    expected_path: &'static str,
    status: u16,
    headers: Vec<(&'static str, &'static str)>,
    body: String,
}

#[test]
fn http_get_supports_expectations() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/health?source=cli",
        status: 200,
        headers: vec![("Content-Type", "application/json"), ("X-Env", "dev")],
        body: "{\"status\":\"ok\"}\n".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "get",
        &format!("{base_url}/health"),
        "--query",
        "source=cli",
        "--expect-status",
        "200",
        "--expect-header",
        "x-env: dev",
        "--expect-body-contains",
        "ok",
        "--expect-json",
        "status:eq:ok",
    ])
    .assert()
    .success()
    .stdout(contains("\"status\":\"ok\""))
    .stdout(contains("\u{1b}").not());

    handle.join().expect("server thread should finish");
}

#[test]
fn http_get_retries_server_errors() {
    let responses = vec![
        MockResponse {
            expected_method: "GET",
            expected_path: "/unstable",
            status: 503,
            headers: vec![("Content-Type", "text/plain")],
            body: "unavailable".to_owned(),
        },
        MockResponse {
            expected_method: "GET",
            expected_path: "/unstable",
            status: 200,
            headers: vec![("Content-Type", "text/plain")],
            body: "ready".to_owned(),
        },
    ];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "get",
        &format!("{base_url}/unstable"),
        "--retry",
        "1",
        "--expect-status",
        "200",
    ])
    .assert()
    .success()
    .stdout(contains("ready"));

    handle.join().expect("server thread should finish");
}

#[test]
fn http_get_does_not_retry_client_errors() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/missing",
        status: 404,
        headers: vec![("Content-Type", "text/plain")],
        body: "missing".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "get",
        &format!("{base_url}/missing"),
        "--retry",
        "2",
        "--expect-status",
        "404",
    ])
    .assert()
    .success();

    handle.join().expect("server thread should finish");
}

#[test]
fn http_get_does_not_retry_assertion_failures() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/healthy",
        status: 200,
        headers: vec![("Content-Type", "text/plain")],
        body: "ready".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "get",
        &format!("{base_url}/healthy"),
        "--retry",
        "2",
        "--expect-body-contains",
        "missing",
    ])
    .assert()
    .failure()
    .stderr(contains("request expectations failed"));

    handle.join().expect("server thread should finish");
}

#[test]
fn http_get_bounds_oversized_response_body() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/large",
        status: 200,
        headers: vec![("Content-Type", "text/plain")],
        body: "abcdefghijklmnop".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let assert = cmd
        .args([
            "--json",
            "http",
            "get",
            &format!("{base_url}/large"),
            "--max-response-bytes",
            "8",
            "--expect-status",
            "200",
        ])
        .assert()
        .success();
    let payload: serde_json::Value =
        serde_json::from_slice(&assert.get_output().stdout).expect("valid JSON output");
    assert_eq!(payload["body"], "abcdefgh");
    assert_eq!(payload["body_truncated"], true);
    assert_eq!(payload["truncated"], true);

    handle.join().expect("server thread should finish");
}

#[test]
fn http_body_assertion_fails_when_response_is_truncated() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/large",
        status: 200,
        headers: vec![("Content-Type", "text/plain")],
        body: "abcdefghijklmnop".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "--json",
        "http",
        "get",
        &format!("{base_url}/large"),
        "--max-response-bytes",
        "8",
        "--expect-body-contains",
        "abc",
    ])
    .assert()
    .failure()
    .stdout(contains("response body was truncated"))
    .stderr(contains("\"code\": \"HTTP_ASSERTION_FAILED\""));

    handle.join().expect("server thread should finish");
}

#[test]
fn http_assert_json_report_is_machine_readable() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/health",
        status: 200,
        headers: vec![("Content-Type", "application/json")],
        body: "{\"status\":\"ok\"}".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let spec_path = temp_dir.path().join("health.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"
version: 1
defaults:
  base_url: {base_url}
  max_response_bytes: 1024
cases:
  - name: health
    request:
      method: GET
      path: /health
    expect:
      status: 200
      json:
        - path: status
          eq: ok
"#
        ),
    )
    .expect("spec file should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "assert",
        &spec_path.to_string_lossy(),
        "--report",
        "json",
    ])
    .assert()
    .success()
    .stdout(contains("\"command\": \"http.assert\""))
    .stdout(contains("\"failed\": 0"))
    .stdout(contains("\u{1b}").not());

    handle.join().expect("server thread should finish");
}

#[test]
fn http_assert_extracts_json_headers_and_text_across_cases() {
    let responses = vec![
        MockResponse {
            expected_method: "GET",
            expected_path: "/session",
            status: 200,
            headers: vec![
                ("Content-Type", "application/json"),
                ("X-Request-Id", "req-7"),
            ],
            body: "{\"data\":{\"token\":\"secret-token\"},\"next\":\"/users/42\"}".to_owned(),
        },
        MockResponse {
            expected_method: "GET",
            expected_path: "/users/42?request=req-7",
            status: 200,
            headers: vec![("Content-Type", "application/json")],
            body: "{\"authorized\":true}".to_owned(),
        },
    ];
    let (base_url, handle) = spawn_mock_server(responses);
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let spec_path = temp_dir.path().join("extract.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"
version: 1
defaults:
  base_url: {base_url}
cases:
  - name: create session
    request:
      path: /session
    expect:
      status: 200
    extract:
      token:
        json: data.token
      request_id:
        header: X-Request-Id
      user_path:
        text:
          regex: '"next":"([^"]+)"'
          group: 1
  - name: use session
    request:
      path: '{{{{user_path}}}}'
      query:
        request: '{{{{request_id}}}}'
      headers:
        authorization: 'Bearer {{{{token}}}}'
    expect:
      status: 200
"#
        ),
    )
    .expect("spec file should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "assert",
        &spec_path.to_string_lossy(),
        "--report",
        "json",
    ])
    .assert()
    .success()
    .stdout(contains("\"failed\": 0"))
    .stdout(contains("secret-token").not());

    handle.join().expect("server thread should finish");
}

#[test]
fn http_assert_does_not_publish_partial_extraction() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/session",
        status: 200,
        headers: vec![("Content-Type", "application/json")],
        body: "{\"token\":\"secret-token\"}".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let spec_path = temp_dir.path().join("atomic-extract.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"
version: 1
defaults:
  base_url: {base_url}
cases:
  - name: incomplete session
    request:
      path: /session
    expect:
      status: 200
    extract:
      token:
        json: token
      missing:
        header: X-Missing
"#
        ),
    )
    .expect("spec file should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args([
        "http",
        "assert",
        &spec_path.to_string_lossy(),
        "--report",
        "json",
    ])
    .assert()
    .failure()
    .stdout(contains("extract 'missing' failed"))
    .stdout(contains("secret-token").not());

    handle.join().expect("server thread should finish");
}

#[test]
fn http_run_alias_fails_on_assertion_error() {
    let responses = vec![MockResponse {
        expected_method: "GET",
        expected_path: "/health",
        status: 500,
        headers: vec![("Content-Type", "application/json")],
        body: "{\"status\":\"error\"}".to_owned(),
    }];
    let (base_url, handle) = spawn_mock_server(responses);
    let temp_dir = TempDir::new().expect("temp dir should exist");
    let spec_path = temp_dir.path().join("failure.yaml");
    std::fs::write(
        &spec_path,
        format!(
            r#"
version: 1
defaults:
  base_url: {base_url}
cases:
  - name: unhealthy
    request:
      method: GET
      path: /health
    expect:
      status: 200
"#
        ),
    )
    .expect("spec file should be written");

    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    cmd.args(["http", "run", &spec_path.to_string_lossy(), "--fail-fast"])
        .assert()
        .failure()
        .stdout(contains("FAIL unhealthy"))
        .stdout(contains("\u{1b}").not())
        .stderr(contains("ah: 1 of 1 HTTP assertion case(s) failed"))
        .stderr(contains("HTTP_ASSERTION_FAILED").not());

    handle.join().expect("server thread should finish");
}

fn spawn_mock_server(responses: Vec<MockResponse>) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should expose local address");
    let base_url = format!("http://{address}");

    let handle = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().expect("request should be accepted");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("read timeout should be set");
            let request = read_http_request(&mut stream);
            let first_line = request
                .lines()
                .next()
                .expect("request should contain start line");
            let mut parts = first_line.split_whitespace();
            let method = parts.next().expect("method should exist");
            let path = parts.next().expect("path should exist");
            assert_eq!(
                method, response.expected_method,
                "unexpected HTTP method in mock request"
            );
            assert_eq!(
                path, response.expected_path,
                "unexpected HTTP path in mock request"
            );

            let mut payload = format!(
                "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                response.status,
                status_reason(response.status),
                response.body.len()
            );
            for (name, value) in &response.headers {
                payload.push_str(name);
                payload.push_str(": ");
                payload.push_str(value);
                payload.push_str("\r\n");
            }
            payload.push_str("\r\n");
            payload.push_str(&response.body);
            stream
                .write_all(payload.as_bytes())
                .expect("response should be written");
            stream.flush().expect("response should be flushed");
        }
    });

    (base_url, handle)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let read = stream
            .read(&mut chunk)
            .expect("request bytes should be readable");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            break;
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
