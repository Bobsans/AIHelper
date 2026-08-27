//! Building and sending the requests, and reading a log archive back.
//!
//! One `JsonApi` describes the transport, the error codes and the headers, and
//! everything else here goes through it. The log download is the exception that
//! is not JSON: an archive with two byte budgets, one for the compressed
//! download and one spent across every entry read out of it.

use super::*;

pub(crate) fn get_issue(
    context: &GithubContext,
    number: u64,
) -> Result<IssueResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/issues/{number}",
        context.repo.owner, context.repo.repo
    );
    github_json::<IssueResponse>(context, Method::GET, &path, None)
}

pub(crate) fn create_issue_comment(
    context: &GithubContext,
    number: u64,
    body: String,
) -> Result<IssueCommentResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/issues/{number}/comments",
        context.repo.owner, context.repo.repo
    );
    github_json::<IssueCommentResponse>(context, Method::POST, &path, Some(json!({ "body": body })))
}

pub(crate) fn list_github_issues(
    context: &GithubContext,
    args: &IssuesArgs,
    target: usize,
) -> Result<Vec<IssueResponse>, InvocationResponse> {
    let per_page = target.min(100);
    let mut page = 1usize;
    let mut issues = Vec::with_capacity(target);

    while issues.len() < target {
        let path = github_issues_list_path(context, args, per_page, page);
        let page_items = github_json::<Vec<IssueResponse>>(context, Method::GET, &path, None)?;
        let page_len = page_items.len();
        issues.extend(
            page_items
                .into_iter()
                .filter(|issue| issue.pull_request.is_none())
                .take(target - issues.len()),
        );
        if page_len < per_page {
            break;
        }
        page = page.saturating_add(1);
    }

    Ok(issues)
}

pub(crate) fn github_issues_list_path(
    context: &GithubContext,
    args: &IssuesArgs,
    per_page: usize,
    page: usize,
) -> String {
    let mut query = vec![
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
        query.push(format!("assignee={}", urlencoding::encode(assignee)));
    }
    if let Some(author) = &args.author {
        query.push(format!("creator={}", urlencoding::encode(author)));
    }
    if let Some(since) = &args.since {
        query.push(format!("since={}", urlencoding::encode(since)));
    }
    let mut path = format!(
        "/repos/{}/{}/issues?{}",
        context.repo.owner,
        context.repo.repo,
        query.join("&")
    );
    if page > 1 {
        path.push_str(&format!("&page={page}"));
    }
    path
}

pub(crate) fn github_issue_search_path(
    context: &GithubContext,
    args: &IssuesArgs,
    search: &str,
    per_page: usize,
) -> String {
    let mut qualifiers = vec![
        format!("repo:{}/{}", context.repo.owner, context.repo.repo),
        "is:issue".to_owned(),
        search.to_owned(),
    ];
    if args.state != "all" {
        qualifiers.push(format!("state:{}", args.state));
    }
    for label in &args.labels {
        qualifiers.push(format!("label:\"{label}\""));
    }
    if let Some(assignee) = &args.assignee {
        qualifiers.push(format!("assignee:{assignee}"));
    }
    if let Some(author) = &args.author {
        qualifiers.push(format!("author:{author}"));
    }
    if let Some(since) = &args.since {
        qualifiers.push(format!("updated:>={since}"));
    }
    format!(
        "/search/issues?q={}&per_page={per_page}",
        urlencoding::encode(&qualifiers.join(" "))
    )
}

pub(crate) fn get_release(
    context: &GithubContext,
    tag: &str,
) -> Result<ReleaseResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/releases/tags/{}",
        context.repo.owner,
        context.repo.repo,
        urlencoding::encode(tag)
    );
    github_json::<ReleaseResponse>(context, Method::GET, &path, None)
}

pub(crate) fn get_run(
    context: &GithubContext,
    run_id: u64,
) -> Result<WorkflowRunResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}",
        context.repo.owner, context.repo.repo
    );
    github_json::<WorkflowRunResponse>(context, Method::GET, &path, None)
}

/// The GitHub REST API as this plugin talks to it.
pub(crate) fn api(context: &GithubContext) -> http::JsonApi<'_> {
    http::JsonApi {
        client: &context.client,
        base_url: &context.api_url,
        service: "GitHub",
        codes: http::ApiErrorCodes {
            transport: "GITHUB_HTTP_FAILED",
            status: "GITHUB_API_FAILED",
            decode: "GITHUB_RESPONSE_INVALID",
        },
        headers: &[
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
            ("User-Agent", "AIHelper-github-plugin"),
        ],
        // Every URL is built from `api_url`, which `resolve_token` already
        // accepted as the token's target, so there is nothing further to bind to.
        authorize: context.token.as_deref().map(|token| http::Authorization {
            scheme: http::AuthScheme::Bearer(token),
            authority: None,
        }),
        error_body_chars: 500,
    }
}

pub(crate) fn github_json<T>(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    api(context).json(method, path, body.as_ref())
}

pub(crate) fn github_no_content(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(), InvocationResponse> {
    api(context).send(method, path, body.as_ref()).map(drop)
}

pub(crate) fn github_response(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::blocking::Response, InvocationResponse> {
    api(context).send(method, path, body.as_ref())
}

/// The archive is downloaded whole, then each entry is scanned. Two budgets
/// apply: `--max-body-bytes` to the compressed download and
/// `--max-expanded-bytes` to everything read out of it, the latter spent across
/// every entry rather than per entry.
pub(crate) fn download_run_logs(
    context: &GithubContext,
    run_id: u64,
    grep: Option<&str>,
    warnings_only: bool,
    line_limit: Option<usize>,
    max_body_bytes: usize,
    max_expanded_bytes: usize,
) -> Result<(Vec<LogLine>, bool), InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/logs",
        context.repo.owner, context.repo.repo
    );
    let response = github_response(context, Method::GET, &path, None)?;
    let bytes = read_bounded_log_body(response, run_id, max_body_bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to open log archive for run {run_id}: {error}"),
        )
    })?;
    let filter = logs::LineFilter {
        grep,
        warnings_only,
        limit: line_limit,
    };
    let mut expanded = logs::ByteBudget::new(max_expanded_bytes);
    let mut matches = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| {
            InvocationResponse::error(
                "GITHUB_RESPONSE_INVALID",
                format!("failed to read log archive entry {index}: {error}"),
            )
        })?;
        if file.is_dir() {
            continue;
        }
        let file_name = file.name().to_owned();
        let mut reader = BufReader::new(file);
        let scan = logs::scan_lines(
            &mut reader,
            &mut expanded,
            &filter,
            &mut matches,
            |line, text| LogLine {
                file: file_name.clone(),
                line,
                text,
            },
        )
        .map_err(|error| expanded_log_failure(index, max_expanded_bytes, error))?;
        if scan.truncated() {
            return Ok((matches, true));
        }
    }
    Ok((matches, false))
}

pub(crate) fn expanded_log_failure(
    index: usize,
    max_expanded_bytes: usize,
    error: logs::ScanError,
) -> InvocationResponse {
    match error {
        logs::ScanError::BudgetExceeded => InvocationResponse::error(
            "GITHUB_RESPONSE_TOO_LARGE",
            format!("expanded workflow logs exceed --max-expanded-bytes {max_expanded_bytes}"),
        ),
        logs::ScanError::Read(error) => InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to read log archive entry {index}: {error}"),
        ),
    }
}

pub(crate) fn read_bounded_log_body(
    mut response: reqwest::blocking::Response,
    run_id: u64,
    max_body_bytes: usize,
) -> Result<Vec<u8>, InvocationResponse> {
    let too_large = || {
        InvocationResponse::error(
            "GITHUB_RESPONSE_TOO_LARGE",
            format!("workflow log archive exceeds --max-body-bytes {max_body_bytes}"),
        )
    };
    let budget = logs::ByteBudget::new(max_body_bytes);
    if !budget.admits(response.content_length()) {
        return Err(too_large());
    }

    let mut bytes = Vec::with_capacity(max_body_bytes.min(64 * 1024));
    response
        .by_ref()
        .take(max_body_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            InvocationResponse::error(
                "GITHUB_RESPONSE_INVALID",
                format!("failed to read log archive for run {run_id}: {error}"),
            )
        })?;
    if bytes.len() > max_body_bytes {
        return Err(too_large());
    }
    Ok(bytes)
}
