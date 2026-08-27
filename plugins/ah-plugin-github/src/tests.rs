//! `github` end to end: argv in, a mock server answering, the response out.
//!
//! These are the tests that go through the real entry points - `parse_args`
//! then `execute` - rather than through one function, so they live next to
//! neither the arguments nor the commands. What each *unit* does is tested in
//! its own module.

use std::io::Write;

use ah_plugin_testkit::{CapturedRequest, MockResponse, MockServer};

use super::*;

#[test]
fn repo_command_falls_back_when_api_lookup_fails() {
    let server = MockServer::new(vec![MockResponse::json(
        500,
        r#"{"message":"server error"}"#,
    )]);

    let response = invoke_json(&["--repo", "acme/tool", "--api-url", &server.url(), "repo"]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.repo");
    assert_eq!(payload["repository"], "acme/tool");
    assert!(payload["html_url"].is_null());
    assert!(payload["default_branch"].is_null());
    assert!(payload["private"].is_null());
}

#[test]
fn issues_list_uses_filters_and_limit() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"[{
            "number": 12,
            "title": "Fix build",
            "body": "body",
            "state": "open",
            "html_url": "https://github.com/acme/tool/issues/12",
            "user": {"login": "alice"},
            "labels": [{"name": "bug"}],
            "assignees": [{"login": "bob"}],
            "comments": 1,
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z",
            "closed_at": null
        }]"#,
    )]);

    let response = invoke_json_with_limit(
        &[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "issues",
            "--state",
            "all",
            "--label",
            "bug",
            "--assignee",
            "bob",
            "--author",
            "alice",
            "--since",
            "2026-05-07T00:00:00Z",
        ],
        Some(5),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.issues");
    assert_eq!(payload["issue_count"], 1);
    let request = only_request(&server);
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.path,
        "/repos/acme/tool/issues?state=all&per_page=5&labels=bug&assignee=bob&creator=alice&since=2026-05-07T00%3A00%3A00Z"
    );
}

#[test]
fn issues_list_pages_past_pull_requests() {
    let first_page = format!(
        "[{},{}]",
        pull_request_issue_json(1),
        pull_request_issue_json(2)
    );
    let second_page = format!("[{},{}]", issue_json(3, "open"), issue_json(4, "open"));
    let server = MockServer::new(vec![
        MockResponse::json(200, &first_page),
        MockResponse::json(200, &second_page),
    ]);

    let response = invoke_json_with_limit(
        &["--repo", "acme/tool", "--api-url", &server.url(), "issues"],
        Some(2),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["issue_count"], 2);
    assert_eq!(payload["issues"][0]["number"], 3);
    assert_eq!(payload["issues"][1]["number"], 4);
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].path,
        "/repos/acme/tool/issues?state=open&per_page=2"
    );
    assert_eq!(
        requests[1].path,
        "/repos/acme/tool/issues?state=open&per_page=2&page=2"
    );
}

#[test]
fn issues_search_uses_search_api() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{"items": [{
            "number": 13,
            "title": "Crash on startup",
            "body": "body",
            "state": "open",
            "html_url": "https://github.com/acme/tool/issues/13",
            "user": {"login": "alice"},
            "labels": [],
            "assignees": [],
            "comments": 0,
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z",
            "closed_at": null
        }]}"#,
    )]);

    let response = invoke_json_with_limit(
        &[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "issues",
            "--search",
            "startup crash",
        ],
        Some(3),
    );

    assert!(response.success, "{response:?}");
    let request = only_request(&server);
    assert!(request.path.starts_with("/search/issues?q="));
    assert!(request.path.contains("per_page=3"));
}

#[test]
fn issue_view_uses_expected_request() {
    let server = MockServer::new(vec![MockResponse::json(200, &issue_json(21, "open"))]);
    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "issue",
        "view",
        "21",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.issue.view");
    assert_eq!(payload["issue"]["number"], 21);
    let request = only_request(&server);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/repos/acme/tool/issues/21");
}

#[test]
fn issue_create_and_update_send_expected_bodies() {
    let create_server = MockServer::new(vec![MockResponse::json(201, &issue_json(21, "open"))]);
    let create_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &create_server.url(),
        "issue",
        "create",
        "--title",
        "Fix build",
        "--body",
        "details",
        "--label",
        "bug",
        "--assignee",
        "bob",
    ]);
    assert!(create_response.success, "{create_response:?}");
    let create_request = only_request(&create_server);
    assert_eq!(create_request.method, "POST");
    assert_eq!(create_request.path, "/repos/acme/tool/issues");
    let create_body: Value =
        serde_json::from_str(&create_request.body).expect("body should be json");
    assert_eq!(create_body["title"], "Fix build");
    assert_eq!(create_body["body"], "details");
    assert_eq!(create_body["labels"][0], "bug");
    assert_eq!(create_body["assignees"][0], "bob");

    let update_server = MockServer::new(vec![MockResponse::json(200, &issue_json(21, "closed"))]);
    let update_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &update_server.url(),
        "issue",
        "update",
        "21",
        "--state",
        "closed",
        "--label",
        "fixed",
    ]);
    assert!(update_response.success, "{update_response:?}");
    let update_request = only_request(&update_server);
    assert_eq!(update_request.method, "PATCH");
    assert_eq!(update_request.path, "/repos/acme/tool/issues/21");
    let update_body: Value =
        serde_json::from_str(&update_request.body).expect("body should be json");
    assert_eq!(update_body["state"], "closed");
    assert_eq!(update_body["labels"][0], "fixed");
}

#[test]
fn issue_close_comments_then_closes() {
    let server = MockServer::new(vec![
        MockResponse::json(201, &issue_comment_json(101)),
        MockResponse::json(200, &issue_json(21, "closed")),
    ]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "issue",
        "close",
        "21",
        "--comment",
        "fixed",
    ]);

    assert!(response.success, "{response:?}");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/repos/acme/tool/issues/21/comments");
    assert_eq!(requests[1].method, "PATCH");
    assert_eq!(requests[1].path, "/repos/acme/tool/issues/21");
}

#[test]
fn issue_comment_and_comments_work() {
    let comment_server = MockServer::new(vec![MockResponse::json(201, &issue_comment_json(101))]);
    let comment_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &comment_server.url(),
        "issue",
        "comment",
        "21",
        "--body",
        "hello",
    ]);
    assert!(comment_response.success, "{comment_response:?}");
    let comment_payload = response_json(&comment_response);
    assert_eq!(comment_payload["command"], "github.issue.comment");

    let list_server = MockServer::new(vec![MockResponse::json(
        200,
        &format!("[{}]", issue_comment_json(101)),
    )]);
    let list_response = invoke_json_with_limit(
        &[
            "--repo",
            "acme/tool",
            "--api-url",
            &list_server.url(),
            "issue",
            "comments",
            "21",
        ],
        Some(2),
    );
    assert!(list_response.success, "{list_response:?}");
    let list_payload = response_json(&list_response);
    assert_eq!(list_payload["command"], "github.issue.comments");
    assert_eq!(list_payload["comment_count"], 1);
    assert_eq!(
        only_request(&list_server).path,
        "/repos/acme/tool/issues/21/comments?per_page=2"
    );
}

#[test]
fn release_get_uses_expected_request_and_auth_header() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "id": 10,
            "tag_name": "v1.0.0",
            "name": "v1.0.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/acme/tool/releases/tag/v1.0.0",
            "published_at": "2026-05-06T00:00:00Z",
            "assets": []
        }"#,
    )]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "--token",
        "secret-token",
        "release",
        "get",
        "v1.0.0",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.release.get");
    assert_eq!(payload["repository"], "acme/tool");
    assert_eq!(payload["release"]["tag_name"], "v1.0.0");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/repos/acme/tool/releases/tags/v1.0.0");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer secret-token")
    );
    assert_eq!(
        requests[0].header("x-github-api-version"),
        Some("2022-11-28")
    );
}

#[test]
fn release_assets_returns_asset_list() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "id": 10,
            "tag_name": "v1.0.0",
            "name": "v1.0.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/acme/tool/releases/tag/v1.0.0",
            "published_at": "2026-05-06T00:00:00Z",
            "assets": [
                {
                    "id": 1,
                    "name": "tool-linux.zip",
                    "size": 123,
                    "browser_download_url": "https://example.test/tool-linux.zip"
                }
            ]
        }"#,
    )]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "release",
        "assets",
        "v1.0.0",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.release.assets");
    assert_eq!(payload["asset_count"], 1);
    assert_eq!(payload["assets"][0]["name"], "tool-linux.zip");
}

#[test]
fn release_create_posts_expected_body() {
    let server = MockServer::new(vec![MockResponse::json(
        201,
        r#"{
            "id": 11,
            "tag_name": "v1.0.1",
            "name": "v1.0.1",
            "draft": true,
            "prerelease": false,
            "html_url": "https://github.com/acme/tool/releases/tag/v1.0.1",
            "published_at": null,
            "assets": []
        }"#,
    )]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "release",
        "create",
        "v1.0.1",
        "--title",
        "v1.0.1",
        "--notes",
        "release notes",
        "--target",
        "main",
        "--draft",
    ]);

    assert!(response.success, "{response:?}");
    let request = only_request(&server);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/repos/acme/tool/releases");
    let body: Value = serde_json::from_str(&request.body).expect("body should be json");
    assert_eq!(body["tag_name"], "v1.0.1");
    assert_eq!(body["target_commitish"], "main");
    assert_eq!(body["name"], "v1.0.1");
    assert_eq!(body["body"], "release notes");
    assert_eq!(body["draft"], true);
    assert_eq!(body["prerelease"], false);
}

#[test]
fn release_create_omits_options_that_were_not_given() {
    let server = MockServer::new(vec![MockResponse::json(
        201,
        r#"{
            "id": 11,
            "tag_name": "v1.0.1",
            "name": null,
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/acme/tool/releases/tag/v1.0.1",
            "published_at": null,
            "assets": []
        }"#,
    )]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "release",
        "create",
        "v1.0.1",
    ]);

    assert!(response.success, "{response:?}");
    let body: Value =
        serde_json::from_str(&only_request(&server).body).expect("body should be json");
    assert_eq!(body["tag_name"], "v1.0.1");
    for key in ["target_commitish", "name", "body"] {
        assert!(body.get(key).is_none(), "{key} should be omitted: {body}");
    }
}

#[test]
fn workflow_dispatch_posts_ref_and_inputs() {
    let server = MockServer::new(vec![MockResponse::empty(204)]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "workflow",
        "run",
        "release.yml",
        "--ref",
        "main",
        "--input",
        "dry_run=true",
    ]);

    assert!(response.success, "{response:?}");
    let request = only_request(&server);
    assert_eq!(request.method, "POST");
    assert_eq!(
        request.path,
        "/repos/acme/tool/actions/workflows/release.yml/dispatches"
    );
    let body: Value = serde_json::from_str(&request.body).expect("body should be json");
    assert_eq!(body["ref"], "main");
    assert_eq!(body["inputs"]["dry_run"], "true");
}

#[test]
fn runs_command_includes_workflow_branch_and_limit() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "workflow_runs": [
                {
                    "id": 42,
                    "name": "CI",
                    "event": "push",
                    "status": "completed",
                    "conclusion": "success",
                    "head_branch": "main",
                    "head_sha": "abc123",
                    "html_url": "https://github.com/acme/tool/actions/runs/42",
                    "created_at": "2026-05-06T00:00:00Z",
                    "updated_at": "2026-05-06T00:01:00Z"
                }
            ]
        }"#,
    )]);

    let response = invoke_json_with_limit(
        &[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "runs",
            "--workflow",
            "ci.yml",
            "--branch",
            "main",
        ],
        Some(3),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["run_count"], 1);
    let request = only_request(&server);
    assert_eq!(
        request.path,
        "/repos/acme/tool/actions/workflows/ci.yml/runs?per_page=3&branch=main"
    );
}

#[test]
fn run_wait_polls_until_completed() {
    let server = MockServer::new(vec![
        MockResponse::json(200, &workflow_run_json(42, "in_progress", None)),
        MockResponse::json(200, &workflow_run_json(42, "completed", Some("success"))),
    ]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "run",
        "wait",
        "42",
        "--interval-secs",
        "1",
        "--timeout-secs",
        "5",
        "--fail-on-failure",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.run.wait");
    assert_eq!(payload["run"]["status"], "completed");
    assert_eq!(payload["run"]["conclusion"], "success");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
}

#[test]
fn run_jobs_and_artifacts_decode_lists() {
    let jobs_server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "jobs": [
                {
                    "id": 7,
                    "name": "test",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/acme/tool/actions/jobs/7",
                    "started_at": "2026-05-06T00:00:00Z",
                    "completed_at": "2026-05-06T00:01:00Z"
                }
            ]
        }"#,
    )]);
    let jobs_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &jobs_server.url(),
        "run",
        "jobs",
        "42",
    ]);
    assert!(jobs_response.success, "{jobs_response:?}");
    let jobs_payload = response_json(&jobs_response);
    assert_eq!(jobs_payload["job_count"], 1);
    assert_eq!(jobs_payload["jobs"][0]["name"], "test");

    let artifacts_server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "artifacts": [
                {
                    "id": 8,
                    "name": "ah-linux-x64.zip",
                    "size_in_bytes": 123,
                    "expired": false,
                    "archive_download_url": "https://api.github.com/artifacts/8/zip"
                }
            ]
        }"#,
    )]);
    let artifacts_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &artifacts_server.url(),
        "run",
        "artifacts",
        "42",
    ]);
    assert!(artifacts_response.success, "{artifacts_response:?}");
    let artifacts_payload = response_json(&artifacts_response);
    assert_eq!(artifacts_payload["artifact_count"], 1);
    assert_eq!(
        artifacts_payload["artifacts"][0]["name"],
        "ah-linux-x64.zip"
    );
}

#[test]
fn run_logs_and_warnings_read_zip_archive() {
    let zip_bytes = log_zip_bytes(&[(
        "Build/1_step.txt",
        "normal line\nNode.js 20 actions are deprecated\n\u{1b}[1mwarning: noisy\u{1b}[0m\n",
    )]);
    let server = MockServer::new(vec![MockResponse::bytes(200, "application/zip", zip_bytes)]);

    let response = invoke_json_with_limit(
        &[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "run",
            "warnings",
            "42",
        ],
        Some(10),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "github.run.warnings");
    assert_eq!(payload["match_count"], 2);
    assert_eq!(
        payload["matches"][1]["text"], "warning: noisy",
        "ANSI escape sequences should be stripped"
    );
}

#[test]
fn run_logs_rejects_compressed_and_expanded_overflow() {
    let zip_bytes = log_zip_bytes(&[("Build/step.txt", "0123456789abcdef\n")]);
    let compressed_limit = zip_bytes.len().saturating_sub(1).to_string();
    let compressed_server = MockServer::new(vec![MockResponse::bytes(
        200,
        "application/zip",
        zip_bytes.clone(),
    )]);
    let compressed_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &compressed_server.url(),
        "run",
        "logs",
        "42",
        "--max-body-bytes",
        &compressed_limit,
    ]);
    assert_eq!(
        compressed_response.error_code.as_deref(),
        Some("GITHUB_RESPONSE_TOO_LARGE")
    );

    let expanded_server =
        MockServer::new(vec![MockResponse::bytes(200, "application/zip", zip_bytes)]);
    let expanded_response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &expanded_server.url(),
        "run",
        "logs",
        "42",
        "--max-expanded-bytes",
        "8",
    ]);
    assert_eq!(
        expanded_response.error_code.as_deref(),
        Some("GITHUB_RESPONSE_TOO_LARGE")
    );
}

#[test]
fn github_api_failure_has_stable_error_code() {
    let server = MockServer::new(vec![MockResponse::json(
        404,
        r#"{"message":"Not Found","status":"404"}"#,
    )]);

    let response = invoke_json(&[
        "--repo",
        "acme/tool",
        "--api-url",
        &server.url(),
        "release",
        "get",
        "missing",
    ]);

    assert!(!response.success);
    assert_eq!(response.error_code.as_deref(), Some("GITHUB_API_FAILED"));
    assert!(
        response
            .error_message
            .as_deref()
            .unwrap_or("")
            .contains("HTTP 404")
    );
}

fn invoke_json(argv: &[&str]) -> InvocationResponse {
    invoke_json_with_limit(argv, None)
}

fn invoke_json_with_limit(argv: &[&str], limit: Option<usize>) -> InvocationResponse {
    let mut argv = argv
        .iter()
        .map(|item| (*item).to_owned())
        .collect::<Vec<_>>();
    if !argv.iter().any(|item| item == "--token") {
        argv.splice(0..0, ["--token".to_owned(), "test-token".to_owned()]);
    }
    let request = InvocationRequest {
        resolved_secrets: Default::default(),
        domain: DOMAIN.to_owned(),
        argv,
        globals: GlobalOptionsWire {
            json: true,
            quiet: false,
            limit,
            cwd: None,
        },
    };
    let request_json = serde_json::to_string(&request).expect("request should serialize");
    let request_c = std::ffi::CString::new(request_json).expect("request should be cstring");
    invoke_from_raw(request_c.as_ptr())
}

fn response_json(response: &InvocationResponse) -> Value {
    serde_json::from_str(response.message.as_deref().expect("message should exist"))
        .expect("message should be json")
}

fn workflow_run_json(id: u64, status: &str, conclusion: Option<&str>) -> String {
    let conclusion = conclusion
        .map(|value| format!(r#""{value}""#))
        .unwrap_or_else(|| "null".to_owned());
    format!(
        r#"{{
            "id": {id},
            "name": "CI",
            "event": "push",
            "status": "{status}",
            "conclusion": {conclusion},
            "head_branch": "main",
            "head_sha": "abc123",
            "html_url": "https://github.com/acme/tool/actions/runs/{id}",
            "created_at": "2026-05-06T00:00:00Z",
            "updated_at": "2026-05-06T00:01:00Z"
        }}"#
    )
}

fn issue_json(number: u64, state: &str) -> String {
    format!(
        r#"{{
            "number": {number},
            "title": "Fix build",
            "body": "body",
            "state": "{state}",
            "html_url": "https://github.com/acme/tool/issues/{number}",
            "user": {{"login": "alice"}},
            "labels": [{{"name": "bug"}}],
            "assignees": [{{"login": "bob"}}],
            "comments": 1,
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z",
            "closed_at": null
        }}"#
    )
}

fn pull_request_issue_json(number: u64) -> String {
    let mut value = serde_json::from_str::<Value>(&issue_json(number, "open"))
        .expect("issue fixture should be JSON");
    value["pull_request"] = json!({ "url": format!("https://api.github.com/pulls/{number}") });
    serde_json::to_string(&value).expect("pull request fixture should serialize")
}

fn issue_comment_json(id: u64) -> String {
    format!(
        r#"{{
            "id": {id},
            "body": "hello",
            "html_url": "https://github.com/acme/tool/issues/21#issuecomment-{id}",
            "user": {{"login": "alice"}},
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z"
        }}"#
    )
}

fn only_request(server: &MockServer) -> CapturedRequest {
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    requests[0].clone()
}

fn log_zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    for (path, content) in files {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .expect("zip file should start");
        writer
            .write_all(content.as_bytes())
            .expect("zip content should write");
    }
    writer.finish().expect("zip should finish").into_inner()
}
