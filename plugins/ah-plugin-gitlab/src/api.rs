//! Building and sending the requests, and reading a job trace back.
//!
//! One `JsonApi` describes the transport, the error codes and the headers, and
//! every REST call goes through it. The GraphQL call does not - it posts a
//! query and checks the token against its own URL - and the trace is a plain
//! byte stream read against a budget rather than JSON.

use super::*;

pub(crate) fn get_release(
    context: &GitlabContext,
    tag: &str,
) -> Result<ReleaseResponse, InvocationResponse> {
    let path = format!(
        "/projects/{}/releases/{}",
        context.project.encoded(),
        urlencoding::encode(tag)
    );
    gitlab_json::<ReleaseResponse>(context, Method::GET, &path, None)
}

pub(crate) fn get_pipeline(
    context: &GitlabContext,
    pipeline_id: u64,
) -> Result<PipelineResponse, InvocationResponse> {
    let path = format!(
        "/projects/{}/pipelines/{pipeline_id}",
        context.project.encoded()
    );
    gitlab_json::<PipelineResponse>(context, Method::GET, &path, None)
}

/// The GitLab REST API as this plugin talks to it.
///
/// The token is bound to the authority it was resolved for, so a redirected
/// path never receives it. The GraphQL call is not built here and keeps its own
/// check through `authorized_token`.
pub(crate) fn api(context: &GitlabContext) -> http::JsonApi<'_> {
    http::JsonApi {
        client: &context.client,
        base_url: &context.api_url,
        service: "GitLab",
        codes: http::ApiErrorCodes {
            transport: "GITLAB_HTTP_FAILED",
            status: "GITLAB_API_FAILED",
            decode: "GITLAB_RESPONSE_INVALID",
        },
        headers: &[
            ("Accept", "application/json"),
            ("User-Agent", "AIHelper-gitlab-plugin"),
        ],
        authorize: context
            .token
            .as_deref()
            .zip(context.token_authority.as_deref())
            .map(|(token, authority)| http::Authorization {
                scheme: http::AuthScheme::Header {
                    name: "PRIVATE-TOKEN",
                    token,
                },
                authority: Some(authority),
            }),
        error_body_chars: 500,
    }
}

pub(crate) fn gitlab_json<T>(
    context: &GitlabContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    api(context).json(method, path, body.as_ref())
}

pub(crate) fn gitlab_graphql<T>(
    context: &GitlabContext,
    body: Value,
) -> Result<GraphqlEnvelope<T>, String>
where
    T: DeserializeOwned,
{
    let mut request = context
        .client
        .request(Method::POST, &context.graphql_url)
        .header("Accept", "application/json")
        .header("User-Agent", "AIHelper-gitlab-plugin");
    if let Some(token) = authorized_token(context, &context.graphql_url) {
        request = request.header("PRIVATE-TOKEN", token);
    }
    let response = request
        .json(&body)
        .send()
        .map_err(|error| format!("request to '{}' failed: {error}", context.graphql_url))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "<failed to read response body>".to_owned());
        return Err(format!(
            "GitLab returned HTTP {status} for '{}': {}",
            context.graphql_url,
            render::truncate_for_error(&body, 500)
        ));
    }
    response
        .json::<GraphqlEnvelope<T>>()
        .map_err(|error| format!("failed to decode GitLab GraphQL response: {error}"))
}

pub(crate) fn gitlab_response(
    context: &GitlabContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::blocking::Response, InvocationResponse> {
    api(context).send(method, path, body.as_ref())
}

pub(crate) fn collect_job_trace(
    context: &GitlabContext,
    job_id: u64,
    grep: Option<&str>,
    warnings_only: bool,
    line_limit: Option<usize>,
    max_body_bytes: usize,
) -> Result<(Vec<TraceLine>, bool), InvocationResponse> {
    let path = format!(
        "/projects/{}/jobs/{job_id}/trace",
        context.project.encoded()
    );
    let response = gitlab_response(context, Method::GET, &path, None)?;
    let too_large = || {
        InvocationResponse::error(
            "GITLAB_RESPONSE_TOO_LARGE",
            format!("job trace exceeds --max-body-bytes {max_body_bytes}"),
        )
    };
    let mut budget = logs::ByteBudget::new(max_body_bytes);
    if !budget.admits(response.content_length()) {
        return Err(too_large());
    }

    let mut matches = Vec::new();
    let scan = logs::scan_lines(
        &mut BufReader::new(response),
        &mut budget,
        &logs::LineFilter {
            grep,
            warnings_only,
            limit: line_limit,
        },
        &mut matches,
        |line, text| TraceLine { line, text },
    )
    .map_err(|error| match error {
        logs::ScanError::BudgetExceeded => too_large(),
        logs::ScanError::Read(error) => InvocationResponse::error(
            "GITLAB_RESPONSE_INVALID",
            format!("failed to read job trace for job {job_id}: {error}"),
        ),
    })?;
    Ok((matches, scan.truncated()))
}

pub(crate) fn get_issue(
    context: &GitlabContext,
    iid: u64,
) -> Result<IssueResponse, InvocationResponse> {
    let path = format!("/projects/{}/issues/{iid}", context.project.encoded());
    gitlab_json::<IssueResponse>(context, Method::GET, &path, None)
}

pub(crate) fn get_issue_comments(
    context: &GitlabContext,
    iid: u64,
    per_page: usize,
) -> Result<Vec<IssueNoteResponse>, InvocationResponse> {
    let path = format!(
        "/projects/{}/issues/{iid}/notes?per_page={per_page}&activity_filter=only_comments",
        context.project.encoded()
    );
    gitlab_json::<Vec<IssueNoteResponse>>(context, Method::GET, &path, None)
}

pub(crate) fn get_issue_designs_best_effort(
    context: &GitlabContext,
    iid: u64,
    first: usize,
) -> Result<Vec<IssueDesignResponse>, String> {
    let project_path = graphql_project_path(context)?;
    let envelope = gitlab_graphql::<IssueDesignsGraphqlData>(
        context,
        json!({
            "query": ISSUE_DESIGNS_QUERY,
            "variables": {
                "fullPath": project_path,
                "iid": iid.to_string(),
                "first": first,
            },
        }),
    )?;
    if !envelope.errors.is_empty() {
        let messages = envelope
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("GitLab GraphQL designs query failed: {messages}"));
    }
    let Some(data) = envelope.data else {
        return Err("GitLab GraphQL designs query returned no data".to_owned());
    };
    let designs = data
        .project
        .and_then(|project| project.issue)
        .and_then(|issue| issue.design_collection)
        .and_then(|collection| collection.designs)
        .map(|connection| connection.nodes)
        .unwrap_or_default();
    Ok(designs)
}

pub(crate) fn graphql_project_path(context: &GitlabContext) -> Result<String, String> {
    if context.project.value.contains('/') {
        return Ok(context.project.value.clone());
    }
    let path = format!("/projects/{}", context.project.encoded());
    let project = gitlab_json::<GitlabProjectResponse>(context, Method::GET, &path, None).map_err(
        |error| {
            error
                .error_message
                .unwrap_or_else(|| "failed to resolve project path for GraphQL".to_owned())
        },
    )?;
    project.path_with_namespace.ok_or_else(|| {
        "GitLab project response did not include path_with_namespace for GraphQL designs query"
            .to_owned()
    })
}

pub(crate) fn create_issue_note(
    context: &GitlabContext,
    iid: u64,
    body: String,
) -> Result<IssueNoteResponse, InvocationResponse> {
    let path = format!("/projects/{}/issues/{iid}/notes", context.project.encoded());
    gitlab_json::<IssueNoteResponse>(context, Method::POST, &path, Some(json!({ "body": body })))
}

pub(crate) fn gitlab_issues_list_path(
    context: &GitlabContext,
    args: &IssuesArgs,
    per_page: usize,
) -> String {
    let mut query = vec![
        "scope=all".to_owned(),
        format!("state={}", urlencoding::encode(&args.state)),
        format!("per_page={per_page}"),
    ];
    if !args.labels.is_empty() {
        query.push(format!(
            "labels={}",
            urlencoding::encode(&args.labels.join(","))
        ));
    }
    if let Some(assignee) = &args.assignee {
        query.push(format!(
            "assignee_username={}",
            urlencoding::encode(assignee)
        ));
    }
    if let Some(author) = &args.author {
        query.push(format!("author_username={}", urlencoding::encode(author)));
    }
    if let Some(since) = &args.since {
        query.push(format!("updated_after={}", urlencoding::encode(since)));
    }
    if let Some(search) = &args.search {
        query.push(format!("search={}", urlencoding::encode(search)));
    }
    format!(
        "/projects/{}/issues?{}",
        context.project.encoded(),
        query.join("&")
    )
}
