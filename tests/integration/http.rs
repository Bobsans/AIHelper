use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

use assert_cmd::Command;
use predicates::str::contains;
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
    .stdout(contains("\"status\":\"ok\""));

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
    .stderr(contains("HTTP_ASSERTION_FAILED"));

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
    .stdout(contains("\"failed\": 0"));

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
        .stderr(contains("HTTP_ASSERTION_FAILED"));

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
        _ => "OK",
    }
}
