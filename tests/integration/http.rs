use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
    thread,
    time::Duration,
};

use super::common::IsolatedAhCommand as Command;
use aihelper::harness::Harness;
use aihelper::secrets::{ExplicitMasterKey, NewSecret, VaultStore};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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

const TEST_MASTER_KEY: &str = "1111111111111111111111111111111111111111111111111111111111111111";

/// `http assert` (or `http run`) over a spec file, with its JSON report.
fn assert_report(spec_path: &std::path::Path, command: &str) -> serde_json::Value {
    Harness::new()
        .run(&[
            "http",
            command,
            &spec_path.to_string_lossy(),
            "--report",
            "json",
        ])
        .json()
}

#[test]
fn http_get_uses_vault_basic_credential_without_exposing_it() {
    let username = "vault-user";
    let password = "http-cli-secret-sentinel";
    let credential_id = "http-cli-private-id";
    let expected_authorization = format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    );
    let (url, handle) = spawn_authorized_server(expected_authorization);
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::http_basic(
            credential_id,
            "HTTP CLI",
            username,
            password,
        ))
        .unwrap();

    let assert = cmd
        .env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .env("AH_LOG_UNREDACTED", "1")
        .args([
            "http",
            "get",
            &url,
            "--credential",
            &format!("basic={credential_id}"),
            "--expect-status",
            "200",
        ])
        .assert()
        .success()
        .stdout(contains("vault-auth-ok"));
    assert!(!String::from_utf8_lossy(&assert.get_output().stdout).contains(password));
    assert!(!String::from_utf8_lossy(&assert.get_output().stderr).contains(password));
    handle.join().expect("server thread should finish");

    let logs = fs::read_dir(cmd.config_dir().join("logs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| fs::read_to_string(entry.path()).unwrap())
        .collect::<String>();
    assert!(!logs.contains(credential_id));
    assert!(!logs.contains(password));
}

#[test]
fn http_missing_vault_credential_keeps_error_code_without_exposing_id() {
    let credential_id = "http-missing-private-id";
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();

    cmd.env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "--json",
            "http",
            "get",
            "http://127.0.0.1:1",
            "--credential",
            &format!("basic={credential_id}"),
        ])
        .assert()
        .failure()
        .stderr(contains("SECRET_NOT_FOUND"))
        .stderr(contains(credential_id).not())
        .stdout(contains(credential_id).not());
}

#[test]
fn http_mismatched_vault_credential_redacts_all_kind_metadata() {
    let credential_id = "http-mismatched-private-id";
    let actual_kind = "postgres";
    let accepted_kind = "http-basic";
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::postgres(
            credential_id,
            "Mismatched CLI credential",
            "database-password",
        ))
        .unwrap();

    cmd.env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "--json",
            "http",
            "get",
            "http://127.0.0.1:1",
            "--credential",
            &format!("basic={credential_id}"),
        ])
        .assert()
        .failure()
        .stderr(contains("SECRET_KIND_MISMATCH"))
        .stderr(contains(credential_id).not())
        .stderr(contains(actual_kind).not())
        .stderr(contains(accepted_kind).not())
        .stdout(contains(credential_id).not())
        .stdout(contains(actual_kind).not())
        .stdout(contains(accepted_kind).not());
}

#[test]
fn credentialed_http_assertion_failure_renders_json_response_before_error() {
    let username = "vault-user";
    let password = "http-cli-assertion-secret";
    let credential_id = "http-cli-assertion-id";
    let expected_authorization = format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    );
    let (url, handle) = spawn_authorized_server(expected_authorization);
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::http_basic(
            credential_id,
            "HTTP CLI assertion",
            username,
            password,
        ))
        .unwrap();

    cmd.env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "--json",
            "http",
            "get",
            &url,
            "--credential",
            &format!("basic={credential_id}"),
            "--expect-status",
            "201",
        ])
        .assert()
        .failure()
        .stdout(contains("\"status\": 200"))
        .stdout(contains("\"content-length\": \"13\""))
        .stdout(contains("\"body\": \"vault-auth-ok\""))
        .stdout(contains(credential_id).not())
        .stdout(contains(password).not())
        .stderr(contains("\"code\": \"HTTP_ASSERTION_FAILED\""))
        .stderr(contains(credential_id).not())
        .stderr(contains(password).not());
    handle.join().expect("server thread should finish");
}

#[test]
fn credentialed_http_replay_assertion_failure_renders_json_response_before_error() {
    let username = "vault-replay-user";
    let password = "http-cli-replay-secret";
    let credential_id = "http-cli-replay-id";
    let expected_authorization = format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    );
    let (url, handle) = spawn_authorized_server(expected_authorization);
    let curl = format!("curl {url}");
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::http_basic(
            credential_id,
            "HTTP CLI replay",
            username,
            password,
        ))
        .unwrap();

    cmd.env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args([
            "--json",
            "http",
            "replay",
            "--curl",
            &curl,
            "--credential",
            &format!("basic={credential_id}"),
            "--expect-status",
            "201",
        ])
        .assert()
        .failure()
        .stdout(contains("\"status\": 200"))
        .stdout(contains("\"content-length\": \"13\""))
        .stdout(contains("\"body\": \"vault-auth-ok\""))
        .stdout(contains(credential_id).not())
        .stdout(contains(password).not())
        .stderr(contains("\"code\": \"HTTP_ASSERTION_FAILED\""))
        .stderr(contains(credential_id).not())
        .stderr(contains(password).not());
    handle.join().expect("server thread should finish");
}

#[test]
fn credentialed_http_quiet_get_assertion_failure_stays_nonzero() {
    assert_quiet_credentialed_assertion_failure("get");
}

#[test]
fn credentialed_http_quiet_replay_assertion_failure_stays_nonzero() {
    assert_quiet_credentialed_assertion_failure("replay");
}

fn assert_quiet_credentialed_assertion_failure(command_name: &str) {
    let username = format!("vault-quiet-{command_name}-user");
    let password = format!("http-cli-quiet-{command_name}-secret");
    let credential_id = format!("http-cli-quiet-{command_name}-id");
    let expected_authorization = format!(
        "Basic {}",
        STANDARD.encode(format!("{username}:{password}"))
    );
    let (url, handle) = spawn_authorized_server(expected_authorization);
    let mut cmd = Command::cargo_bin("ah").expect("binary should compile");
    let store = VaultStore::at(
        cmd.config_dir(),
        Arc::new(ExplicitMasterKey::parse(TEST_MASTER_KEY.to_owned()).unwrap()),
    );
    store.initialize().unwrap();
    store
        .put(NewSecret::http_basic(
            &credential_id,
            format!("HTTP quiet {command_name}"),
            &username,
            &password,
        ))
        .unwrap();
    let mut arguments = vec![
        "--json".to_owned(),
        "--quiet".to_owned(),
        "http".to_owned(),
        command_name.to_owned(),
    ];
    if command_name == "replay" {
        arguments.extend(["--curl".to_owned(), format!("curl {url}")]);
    } else {
        arguments.push(url);
    }
    arguments.extend([
        "--credential".to_owned(),
        format!("basic={credential_id}"),
        "--expect-status".to_owned(),
        "201".to_owned(),
    ]);

    cmd.env("APPDATA", "")
        .env("AH_VAULT_MASTER_KEY", TEST_MASTER_KEY)
        .args(arguments)
        .assert()
        .failure()
        .stdout(predicates::str::is_empty())
        .stderr(contains("\"code\": \"HTTP_ASSERTION_FAILED\""))
        .stderr(contains(&credential_id).not())
        .stderr(contains(&password).not());
    handle.join().expect("server thread should finish");
}

#[test]
fn http_credentials_reject_malformed_and_duplicate_slots_without_echoing_ids() {
    let mut malformed = Command::cargo_bin("ah").unwrap();
    malformed
        .env("APPDATA", "")
        .args([
            "http",
            "get",
            "http://127.0.0.1:1",
            "--credential",
            "private-id",
        ])
        .assert()
        .failure()
        .stderr(contains("--credential must use SLOT=ID"))
        .stderr(predicates::str::contains("private-id").not());

    let mut duplicate = Command::cargo_bin("ah").unwrap();
    duplicate
        .env("APPDATA", "")
        .args([
            "http",
            "get",
            "http://127.0.0.1:1",
            "--credential",
            "basic=first-private-id",
            "--credential=basic=second-private-id",
        ])
        .assert()
        .failure()
        .stderr(contains("duplicate credential slot 'basic'"))
        .stderr(predicates::str::contains("first-private-id").not())
        .stderr(predicates::str::contains("second-private-id").not());
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

    let output = Harness::new()
        .run(&[
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
        .expect_success()
        .to_owned();

    assert!(output.contains("\"status\":\"ok\""), "{output}");

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

    let output = Harness::new()
        .run(&[
            "http",
            "get",
            &format!("{base_url}/unstable"),
            "--retry",
            "1",
            "--expect-status",
            "200",
        ])
        .expect_success()
        .to_owned();

    assert!(output.contains("ready"), "{output}");

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

    // One queued response: a retry would find the server gone.
    let _ = Harness::new()
        .run(&[
            "http",
            "get",
            &format!("{base_url}/missing"),
            "--retry",
            "2",
            "--expect-status",
            "404",
        ])
        .expect_success();

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

    let run = Harness::new().run(&[
        "http",
        "get",
        &format!("{base_url}/healthy"),
        "--retry",
        "2",
        "--expect-body-contains",
        "missing",
    ]);

    let _ = run.expect_failure();
    assert!(
        run.detail().contains("request expectations failed"),
        "{}",
        run.detail()
    );

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

    let payload = Harness::new()
        .run(&[
            "--json",
            "http",
            "get",
            &format!("{base_url}/large"),
            "--max-response-bytes",
            "8",
            "--expect-status",
            "200",
        ])
        .json();
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

    let report = assert_report(&spec_path, "assert");

    assert_eq!(report["command"], "http.assert");
    assert_eq!(report["summary"]["failed"], 0);

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

    let run = Harness::new().run(&[
        "http",
        "assert",
        &spec_path.to_string_lossy(),
        "--report",
        "json",
    ]);
    let report = run.json();

    assert_eq!(report["summary"]["failed"], 0);
    assert!(
        !run.expect_success().contains("secret-token"),
        "an extracted value never reaches the report: {}",
        run.expect_success()
    );

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

    let run = Harness::new().run(&[
        "http",
        "assert",
        &spec_path.to_string_lossy(),
        "--report",
        "json",
    ]);

    let _ = run.expect_failure();
    assert!(
        run.stdout().contains("extract 'missing' failed"),
        "{}",
        run.stdout()
    );
    assert!(
        !run.stdout().contains("secret-token"),
        "a partial extraction publishes nothing: {}",
        run.stdout()
    );

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

    let argv = ["http", "run", &spec_path.to_string_lossy(), "--fail-fast"];
    let run = Harness::new().run(&argv);

    let _ = run.expect_failure();
    assert!(run.stdout().contains("FAIL unhealthy"), "{}", run.stdout());
    assert!(
        run.diagnostic_text(&argv)
            .starts_with("ah: 1 of 1 HTTP assertion case(s) failed"),
        "{}",
        run.diagnostic_text(&argv)
    );
    assert!(
        !run.diagnostic_text(&argv).contains("HTTP_ASSERTION_FAILED"),
        "no internal code leaks into the text"
    );

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

fn spawn_authorized_server(expected_authorization: String) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should expose local address");
    let url = format!("http://{address}/vault-auth");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request should be accepted");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("read timeout should be set");
        let request = read_http_request(&mut stream);
        assert!(
            request.lines().any(|line| {
                line.split_once(':').is_some_and(|(name, value)| {
                    name.eq_ignore_ascii_case("authorization")
                        && value.trim() == expected_authorization
                })
            }),
            "request must contain the resolved Basic authorization header"
        );
        let body = "vault-auth-ok";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response should be written");
    });
    (url, handle)
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
