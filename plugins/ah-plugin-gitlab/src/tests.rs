//! `gitlab` end to end: argv in, a mock server answering, the response out.
//!
//! These are the tests that go through the real entry points - `parse_args`
//! then `execute` - rather than through one function, so they live next to
//! neither the arguments nor the commands. What each *unit* does is tested in
//! its own module.

use std::time::Instant;

use ah_plugin_testkit::{CapturedRequest, MockResponse, MockServer};

use super::*;

#[test]
fn release_get_uses_encoded_project_and_private_token() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"{
            "tag_name": "v1.0.0",
            "name": "v1.0.0",
            "description": "notes",
            "created_at": "2026-05-07T00:00:00Z",
            "released_at": "2026-05-07T00:01:00Z",
            "upcoming_release": false,
            "assets": {"links": []}
        }"#,
    )]);

    let response = invoke_json(&[
        "--project",
        "group/subgroup/tool",
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
    assert_eq!(payload["command"], "gitlab.release.get");
    assert_eq!(payload["project"], "group/subgroup/tool");
    assert_eq!(payload["release"]["tag_name"], "v1.0.0");

    let request = only_request(&server);
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.path,
        "/projects/group%2Fsubgroup%2Ftool/releases/v1.0.0"
    );
    assert_eq!(request.header("private-token"), Some("secret-token"));
}

#[test]
fn release_create_posts_expected_body() {
    let server = MockServer::new(vec![MockResponse::json(
        201,
        r#"{
            "tag_name": "v1.0.1",
            "name": "v1.0.1",
            "description": "release notes",
            "created_at": "2026-05-07T00:00:00Z",
            "released_at": "2026-05-07T00:01:00Z",
            "upcoming_release": false,
            "assets": {"links": []}
        }"#,
    )]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "release",
        "create",
        "v1.0.1",
        "--name",
        "v1.0.1",
        "--description",
        "release notes",
        "--ref",
        "main",
    ]);

    assert!(response.success, "{response:?}");
    let request = only_request(&server);
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/projects/group%2Ftool/releases");
    let body: Value = serde_json::from_str(&request.body).expect("body should be json");
    assert_eq!(body["tag_name"], "v1.0.1");
    assert_eq!(body["name"], "v1.0.1");
    assert_eq!(body["description"], "release notes");
    assert_eq!(body["ref"], "main");
}

#[test]
fn release_create_omits_options_that_were_not_given() {
    let server = MockServer::new(vec![MockResponse::json(
        201,
        r#"{
            "tag_name": "v1.0.1",
            "name": "v1.0.1",
            "description": null,
            "created_at": "2026-05-07T00:00:00Z",
            "released_at": null,
            "upcoming_release": false,
            "assets": {"links": []}
        }"#,
    )]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
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
    for key in ["name", "description", "ref"] {
        assert!(body.get(key).is_none(), "{key} should be omitted: {body}");
    }
}

#[test]
fn pipelines_command_includes_ref_and_limit() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"[{
            "id": 42,
            "iid": 3,
            "project_id": 9,
            "sha": "abc123",
            "ref": "main",
            "status": "success",
            "source": "push",
            "web_url": "https://gitlab.example.com/group/tool/-/pipelines/42",
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z"
        }]"#,
    )]);

    let response = invoke_json_with_limit(
        &[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "pipelines",
            "--branch",
            "main",
        ],
        Some(3),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["pipeline_count"], 1);
    let request = only_request(&server);
    assert_eq!(
        request.path,
        "/projects/group%2Ftool/pipelines?per_page=3&ref=main"
    );
}

#[test]
fn pipeline_wait_polls_until_terminal() {
    let server = MockServer::new(vec![
        MockResponse::json(200, pipeline_json(42, "running")),
        MockResponse::json(200, pipeline_json(42, "success")),
    ]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "pipeline",
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
    assert_eq!(payload["command"], "gitlab.pipeline.wait");
    assert_eq!(payload["pipeline"]["status"], "success");
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn pipeline_wait_does_not_poll_after_deadline() {
    let server = MockServer::new(vec![MockResponse::json(200, pipeline_json(42, "running"))]);
    let started = Instant::now();

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "pipeline",
        "wait",
        "42",
        "--interval-secs",
        "60",
        "--timeout-secs",
        "1",
    ]);

    assert_eq!(
        response.error_code.as_deref(),
        Some("GITLAB_PIPELINE_TIMEOUT")
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(server.requests().len(), 1);
}

#[test]
fn pipeline_jobs_decodes_list() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        r#"[{
            "id": 7,
            "name": "test",
            "status": "success",
            "stage": "test",
            "ref": "main",
            "allow_failure": false,
            "web_url": "https://gitlab.example.com/group/tool/-/jobs/7",
            "created_at": "2026-05-07T00:00:00Z",
            "started_at": "2026-05-07T00:00:10Z",
            "finished_at": "2026-05-07T00:01:00Z"
        }]"#,
    )]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "pipeline",
        "jobs",
        "42",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["job_count"], 1);
    assert_eq!(payload["jobs"][0]["name"], "test");
    assert_eq!(
        only_request(&server).path,
        "/projects/group%2Ftool/pipelines/42/jobs?per_page=100"
    );
}

#[test]
fn job_trace_and_warnings_read_plain_text() {
    let server = MockServer::new(vec![MockResponse::bytes(
        200,
        "text/plain",
        "normal line\nwarning: deprecated config\n\u{1b}[1mwill be removed soon\u{1b}[0m\n"
            .as_bytes()
            .to_vec(),
    )]);

    let response = invoke_json_with_limit(
        &[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "job",
            "warnings",
            "7",
        ],
        Some(10),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "gitlab.job.warnings");
    assert_eq!(payload["match_count"], 2);
    assert_eq!(
        payload["matches"][1]["text"], "will be removed soon",
        "ANSI escape sequences should be stripped"
    );
}

#[test]
fn job_trace_rejects_oversized_body() {
    let server = MockServer::new(vec![MockResponse::bytes(
        200,
        "text/plain",
        b"0123456789\n".to_vec(),
    )]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "job",
        "trace",
        "7",
        "--max-body-bytes",
        "5",
    ]);

    assert_eq!(
        response.error_code.as_deref(),
        Some("GITLAB_RESPONSE_TOO_LARGE")
    );
}

#[test]
fn issues_list_uses_filters_and_limit() {
    let server = MockServer::new(vec![MockResponse::json(
        200,
        &format!("[{}]", issue_json(21, "opened")),
    )]);

    let response = invoke_json_with_limit(
        &[
            "--project",
            "group/tool",
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
            "--search",
            "crash",
        ],
        Some(5),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "gitlab.issues");
    assert_eq!(payload["issue_count"], 1);
    let request = only_request(&server);
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.path,
        "/projects/group%2Ftool/issues?scope=all&state=all&per_page=5&labels=bug&assignee_username=bob&author_username=alice&updated_after=2026-05-07T00%3A00%3A00Z&search=crash"
    );
}

#[test]
fn issue_view_uses_expected_request() {
    let server = MockServer::new(vec![MockResponse::json(200, issue_json(21, "opened"))]);
    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "issue",
        "view",
        "21",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "gitlab.issue.view");
    assert_eq!(payload["issue"]["iid"], 21);
    let request = only_request(&server);
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/projects/group%2Ftool/issues/21");
}

#[test]
fn issue_view_full_reads_comments_and_designs() {
    let server = MockServer::new(vec![
        MockResponse::json(200, issue_json(21, "opened")),
        MockResponse::json(200, &format!("[{}]", issue_note_json(101))),
        MockResponse::json(
            200,
            r#"{
                "data": {
                    "project": {
                        "issue": {
                            "designCollection": {
                                "designs": {
                                    "nodes": [{
                                        "id": "gid://gitlab/DesignManagement::Design/1",
                                        "filename": "mockup.png",
                                        "fullPath": "designs/mockup.png",
                                        "image": "https://gitlab.example.com/group/tool/uploads/designs/mockup.png",
                                        "imageV432x230": "https://gitlab.example.com/group/tool/uploads/designs/mockup.thumb.png",
                                        "notesCount": 2,
                                        "event": "NONE",
                                        "upstream_only": "ignored"
                                    }]
                                }
                            }
                        }
                    }
                }
            }"#,
        ),
    ]);

    let response = invoke_json_with_limit(
        &[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "--graphql-url",
            &format!("{}/graphql", server.url()),
            "issue",
            "view",
            "21",
            "--full",
        ],
        Some(2),
    );

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["command"], "gitlab.issue.view");
    assert_eq!(payload["full"], true);
    assert_eq!(payload["comment_count"], 1);
    assert_eq!(payload["comments"][0]["body"], "I can reproduce this");
    assert_eq!(payload["design_count"], 1);
    assert_eq!(payload["designs"][0]["filename"], "mockup.png");
    assert!(payload["issue"].get("upstream_only").is_none());
    assert!(payload["comments"][0].get("upstream_only").is_none());
    assert!(payload["designs"][0].get("upstream_only").is_none());
    assert_eq!(
        payload["warnings"]
            .as_array()
            .expect("warnings array")
            .len(),
        0
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/projects/group%2Ftool/issues/21");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].path,
        "/projects/group%2Ftool/issues/21/notes?per_page=2&activity_filter=only_comments"
    );
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].path, "/graphql");
    assert!(requests[2].body.contains("IssueDesigns"));
    assert!(requests[2].body.contains("\"fullPath\":\"group/tool\""));
    assert!(requests[2].body.contains("\"iid\":\"21\""));
    assert!(requests[2].body.contains("\"first\":2"));
}

#[test]
fn issue_view_full_keeps_issue_when_design_query_fails() {
    let server = MockServer::new(vec![
        MockResponse::json(200, issue_json(21, "opened")),
        MockResponse::json(200, &format!("[{}]", issue_note_json(101))),
        MockResponse::json(
            200,
            r#"{"errors":[{"message":"Field 'designCollection' doesn't exist"}]}"#,
        ),
    ]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "--graphql-url",
        &format!("{}/graphql", server.url()),
        "issue",
        "view",
        "21",
        "--full",
    ]);

    assert!(response.success, "{response:?}");
    let payload = response_json(&response);
    assert_eq!(payload["full"], true);
    assert_eq!(payload["comment_count"], 1);
    assert_eq!(payload["design_count"], 0);
    assert_eq!(
        payload["warnings"][0],
        "GitLab GraphQL designs query failed: Field 'designCollection' doesn't exist"
    );
}

#[test]
fn graphql_url_normalization_is_explicit_and_predictable() {
    assert_eq!(
        normalize_graphql_url(
            Some("https://proxy.example/graphql"),
            "https://gitlab.example/api/v4",
            "https://gitlab.example",
        )
        .expect("explicit GraphQL URL"),
        "https://proxy.example/graphql"
    );
    assert_eq!(
        normalize_graphql_url(
            None,
            "https://gitlab.example/api/v4",
            "https://gitlab.example",
        )
        .expect("standard API suffix"),
        "https://gitlab.example/api/graphql"
    );
    assert_eq!(
        normalize_graphql_url(None, "https://proxy.example/rest", "https://gitlab.example",)
            .expect("host fallback"),
        "https://gitlab.example/api/graphql"
    );
}

#[test]
fn issue_create_and_update_send_expected_bodies() {
    let create_server = MockServer::new(vec![MockResponse::json(201, issue_json(21, "opened"))]);

    let create_response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &create_server.url(),
        "issue",
        "create",
        "--title",
        "Fix build",
        "--description",
        "Build fails",
        "--label",
        "bug",
        "--assignee-id",
        "10",
    ]);

    assert!(create_response.success, "{create_response:?}");
    let create_request = only_request(&create_server);
    assert_eq!(create_request.method, "POST");
    assert_eq!(create_request.path, "/projects/group%2Ftool/issues");
    let create_body: Value =
        serde_json::from_str(&create_request.body).expect("body should be json");
    assert_eq!(create_body["title"], "Fix build");
    assert_eq!(create_body["description"], "Build fails");
    assert_eq!(create_body["labels"], "bug");
    assert_eq!(create_body["assignee_ids"], json!([10]));

    let update_server = MockServer::new(vec![MockResponse::json(200, issue_json(21, "closed"))]);

    let update_response = invoke_json(&[
        "--project",
        "group/tool",
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
    assert_eq!(update_request.method, "PUT");
    assert_eq!(update_request.path, "/projects/group%2Ftool/issues/21");
    let update_body: Value =
        serde_json::from_str(&update_request.body).expect("body should be json");
    assert_eq!(update_body["state_event"], "close");
    assert_eq!(update_body["labels"], "fixed");
}

#[test]
fn issue_close_comments_then_closes() {
    let server = MockServer::new(vec![
        MockResponse::json(201, issue_note_json(101)),
        MockResponse::json(200, issue_json(21, "closed")),
    ]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "issue",
        "close",
        "21",
        "--comment",
        "Fixed in main",
    ]);

    assert!(response.success, "{response:?}");
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/projects/group%2Ftool/issues/21/notes");
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].path, "/projects/group%2Ftool/issues/21");
    let close_body: Value = serde_json::from_str(&requests[1].body).expect("body should be json");
    assert_eq!(close_body["state_event"], "close");
}

#[test]
fn issue_comment_and_comments_work() {
    let comment_server = MockServer::new(vec![MockResponse::json(201, issue_note_json(101))]);

    let comment_response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &comment_server.url(),
        "issue",
        "comment",
        "21",
        "--body",
        "I can reproduce this",
    ]);

    assert!(comment_response.success, "{comment_response:?}");
    let comment_payload = response_json(&comment_response);
    assert_eq!(comment_payload["command"], "gitlab.issue.comment");
    let comment_request = only_request(&comment_server);
    assert_eq!(
        comment_request.path,
        "/projects/group%2Ftool/issues/21/notes"
    );

    let list_server = MockServer::new(vec![MockResponse::json(
        200,
        &format!("[{}]", issue_note_json(101)),
    )]);

    let list_response = invoke_json_with_limit(
        &[
            "--project",
            "group/tool",
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
    assert_eq!(list_payload["command"], "gitlab.issue.comments");
    let list_request = only_request(&list_server);
    assert_eq!(
        list_request.path,
        "/projects/group%2Ftool/issues/21/notes?per_page=2&activity_filter=only_comments"
    );
}

#[test]
fn gitlab_api_failure_has_stable_error_code() {
    let server = MockServer::new(vec![MockResponse::json(
        404,
        r#"{"message":"404 Project Not Found"}"#,
    )]);

    let response = invoke_json(&[
        "--project",
        "group/tool",
        "--api-url",
        &server.url(),
        "release",
        "get",
        "missing",
    ]);

    assert!(!response.success);
    assert_eq!(response.error_code.as_deref(), Some("GITLAB_API_FAILED"));
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

fn pipeline_json(id: u64, status: &str) -> &'static str {
    let raw = format!(
        r#"{{
            "id": {id},
            "iid": 3,
            "project_id": 9,
            "sha": "abc123",
            "ref": "main",
            "status": "{status}",
            "source": "push",
            "web_url": "https://gitlab.example.com/group/tool/-/pipelines/{id}",
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z"
        }}"#
    );
    Box::leak(raw.into_boxed_str())
}

fn issue_json(iid: u64, state: &str) -> &'static str {
    let raw = format!(
        r#"{{
            "id": {iid},
            "iid": {iid},
            "project_id": 9,
            "title": "Fix build",
            "description": "Build fails",
            "state": "{state}",
            "web_url": "https://gitlab.example.com/group/tool/-/issues/{iid}",
            "author": {{"id": 1, "username": "alice", "name": "Alice"}},
            "assignees": [{{"id": 10, "username": "bob", "name": "Bob"}}],
            "labels": ["bug"],
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z",
            "closed_at": null,
            "upstream_only": "ignored"
        }}"#
    );
    Box::leak(raw.into_boxed_str())
}

fn issue_note_json(id: u64) -> &'static str {
    let raw = format!(
        r#"{{
            "id": {id},
            "body": "I can reproduce this",
            "author": {{"id": 10, "username": "bob", "name": "Bob"}},
            "created_at": "2026-05-07T00:00:00Z",
            "updated_at": "2026-05-07T00:01:00Z",
            "system": false,
            "upstream_only": "ignored"
        }}"#
    );
    Box::leak(raw.into_boxed_str())
}

fn only_request(server: &MockServer) -> CapturedRequest {
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    requests[0].clone()
}
