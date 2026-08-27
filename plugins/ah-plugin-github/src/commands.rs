//! One function per command, each turning parsed arguments into a response.
//!
//! This is where the two forge plugins genuinely differ - GitHub pages past
//! pull requests, unzips a log archive, and has artifacts - so it is the file
//! that has no counterpart on the GitLab side.

use super::*;

pub(crate) fn execute_repo(
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!("/repos/{}/{}", context.repo.owner, context.repo.repo);
    let (html_url, default_branch, private) =
        match github_json::<GithubRepoResponse>(context, Method::GET, &path, None) {
            Ok(value) => (value.html_url, value.default_branch, value.private),
            Err(_) => (None, None, None),
        };

    let output = RepoOutput {
        command: "github.repo",
        repository: context.repo.full_name(),
        owner: context.repo.owner.clone(),
        name: context.repo.repo.clone(),
        remote_url: context.remote_url.clone(),
        api_url: context.api_url.clone(),
        html_url,
        default_branch,
        private,
    };

    render::render_success(
        globals,
        &output,
        format!(
            "{}\n",
            TextFormatter::stdout().paint(TextStyle::Key, &output.repository)
        ),
    )
}

pub(crate) fn execute_issues(
    args: IssuesArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let target = globals.limit.unwrap_or(20).clamp(1, 100);
    let issues = if let Some(search) = &args.search {
        let path = github_issue_search_path(context, &args, search, target);
        match github_json::<IssueSearchResponse>(context, Method::GET, &path, None) {
            Ok(value) => value.items.into_iter().take(target).collect(),
            Err(error) => return error,
        }
    } else {
        match list_github_issues(context, &args, target) {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
    let text = render_issues_text(&issues, TextFormatter::stdout());
    render::render_success(
        globals,
        &IssuesOutput {
            command: "github.issues",
            repository: context.repo.full_name(),
            state: args.state,
            labels: args.labels,
            assignee: args.assignee,
            author: args.author,
            since: args.since,
            search: args.search,
            issue_count: issues.len(),
            issues,
        },
        text,
    )
}

pub(crate) fn execute_issue(
    args: IssueArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        IssueCommand::View(args) => {
            let issue = match get_issue(context, args.number) {
                Ok(value) => value,
                Err(error) => return error,
            };
            render::render_success(
                globals,
                &IssueOutput {
                    command: "github.issue.view",
                    repository: context.repo.full_name(),
                    issue: issue.clone(),
                },
                render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
            )
        }
        IssueCommand::Create(args) => create_issue(context, args, globals),
        IssueCommand::Update(args) => update_issue(context, args, globals),
        IssueCommand::Close(args) => close_issue(context, args, globals),
        IssueCommand::Comment(args) => comment_issue(context, args, globals),
        IssueCommand::Comments(args) => issue_comments(context, args.number, globals),
    }
}

pub(crate) fn create_issue(
    context: &GithubContext,
    args: CreateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body_text = match text::optional(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), Value::String(args.title));
    if let Some(body_text) = body_text {
        body.insert("body".to_owned(), Value::String(body_text));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), json!(args.labels));
    }
    if !args.assignees.is_empty() {
        body.insert("assignees".to_owned(), json!(args.assignees));
    }
    let path = format!("/repos/{}/{}/issues", context.repo.owner, context.repo.repo);
    let issue =
        match github_json::<IssueResponse>(context, Method::POST, &path, Some(Value::Object(body)))
        {
            Ok(value) => value,
            Err(error) => return error,
        };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.create",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn update_issue(
    context: &GithubContext,
    args: UpdateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body_text = match text::optional(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    if let Some(title) = args.title {
        body.insert("title".to_owned(), Value::String(title));
    }
    if let Some(body_text) = body_text {
        body.insert("body".to_owned(), Value::String(body_text));
    }
    if let Some(state) = args.state {
        body.insert("state".to_owned(), Value::String(state));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), json!(args.labels));
    }
    if !args.assignees.is_empty() {
        body.insert("assignees".to_owned(), json!(args.assignees));
    }
    if body.is_empty() {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            "issue update requires at least one field",
        );
    }
    let path = format!(
        "/repos/{}/{}/issues/{}",
        context.repo.owner, context.repo.repo, args.number
    );
    let issue = match github_json::<IssueResponse>(
        context,
        Method::PATCH,
        &path,
        Some(Value::Object(body)),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.update",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn close_issue(
    context: &GithubContext,
    args: CloseIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let comment = match text::optional(args.comment, args.comment_file, "comment") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(comment) = comment
        && let Err(error) = create_issue_comment(context, args.number, comment)
    {
        return error;
    }
    let path = format!(
        "/repos/{}/{}/issues/{}",
        context.repo.owner, context.repo.repo, args.number
    );
    let issue = match github_json::<IssueResponse>(
        context,
        Method::PATCH,
        &path,
        Some(json!({ "state": "closed" })),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.close",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn comment_issue(
    context: &GithubContext,
    args: CommentIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body = match text::required(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let comment = match create_issue_comment(context, args.number, body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueCommentOutput {
            command: "github.issue.comment",
            repository: context.repo.full_name(),
            number: args.number,
            comment: comment.clone(),
        },
        render_comments_text(std::slice::from_ref(&comment), TextFormatter::stdout()),
    )
}

pub(crate) fn issue_comments(
    context: &GithubContext,
    number: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = format!(
        "/repos/{}/{}/issues/{number}/comments?per_page={per_page}",
        context.repo.owner, context.repo.repo
    );
    let comments = match github_json::<Vec<IssueCommentResponse>>(context, Method::GET, &path, None)
    {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueCommentsOutput {
            command: "github.issue.comments",
            repository: context.repo.full_name(),
            number,
            comment_count: comments.len(),
            comments: comments.clone(),
        },
        render_comments_text(&comments, TextFormatter::stdout()),
    )
}

pub(crate) fn execute_release(
    args: ReleaseArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        ReleaseCommand::Get(args) => {
            let release = match get_release(context, &args.tag) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_release_text(&release, TextFormatter::stdout());
            render::render_success(
                globals,
                &ReleaseOutput {
                    command: "github.release.get",
                    repository: context.repo.full_name(),
                    release,
                },
                text,
            )
        }
        ReleaseCommand::Assets(args) => {
            let release = match get_release(context, &args.tag) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let assets = release.assets;
            let text = render_assets_text(&assets, TextFormatter::stdout());
            render::render_success(
                globals,
                &ReleaseAssetsOutput {
                    command: "github.release.assets",
                    repository: context.repo.full_name(),
                    tag: args.tag,
                    asset_count: assets.len(),
                    assets,
                },
                text,
            )
        }
        ReleaseCommand::Create(args) => create_release(context, args, globals),
    }
}

pub(crate) fn execute_workflows(
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/workflows?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let workflows = match github_json::<WorkflowListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.workflows,
        Err(error) => return error,
    };
    let text = render_workflows_text(&workflows, TextFormatter::stdout());
    render::render_success(
        globals,
        &WorkflowsOutput {
            command: "github.workflows",
            repository: context.repo.full_name(),
            workflow_count: workflows.len(),
            workflows,
        },
        text,
    )
}

pub(crate) fn execute_workflow(
    args: WorkflowArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        WorkflowCommand::Run(args) => dispatch_workflow(context, args, globals),
    }
}

pub(crate) fn execute_runs(
    args: RunsArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(10).clamp(1, 100);
    let branch_query = args
        .branch
        .as_ref()
        .map(|branch| format!("&branch={}", urlencoding::encode(branch)))
        .unwrap_or_default();
    let path = if let Some(workflow) = &args.workflow {
        format!(
            "/repos/{}/{}/actions/workflows/{}/runs?per_page={per_page}{branch_query}",
            context.repo.owner,
            context.repo.repo,
            urlencoding::encode(workflow)
        )
    } else {
        format!(
            "/repos/{}/{}/actions/runs?per_page={per_page}{branch_query}",
            context.repo.owner, context.repo.repo
        )
    };

    let runs = match github_json::<RunsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.workflow_runs,
        Err(error) => return error,
    };
    let text = render_runs_text(&runs, TextFormatter::stdout());
    render::render_success(
        globals,
        &RunsOutput {
            command: "github.runs",
            repository: context.repo.full_name(),
            workflow: args.workflow,
            branch: args.branch,
            run_count: runs.len(),
            runs,
        },
        text,
    )
}

pub(crate) fn execute_run(
    args: RunArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        RunCommand::Get(args) => {
            let run = match get_run(context, args.run_id) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_runs_text(std::slice::from_ref(&run), TextFormatter::stdout());
            render::render_success(
                globals,
                &RunOutput {
                    command: "github.run.get",
                    repository: context.repo.full_name(),
                    run,
                },
                text,
            )
        }
        RunCommand::Wait(args) => wait_run(context, args, globals),
        RunCommand::Jobs(args) => run_jobs(context, args.run_id, globals),
        RunCommand::Logs(args) => {
            run_logs(context, args.run_id, args.grep, args.limits, globals, false)
        }
        RunCommand::Warnings(args) => {
            run_logs(context, args.run_id, None, args.limits, globals, true)
        }
        RunCommand::Artifacts(args) => run_artifacts(context, args.run_id, globals),
    }
}

pub(crate) fn create_release(
    context: &GithubContext,
    args: CreateReleaseArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let notes = match (args.notes, args.notes_file) {
        (Some(notes), None) => Some(notes),
        (None, Some(path)) => match fs::read_to_string(&path) {
            Ok(value) => Some(value),
            Err(error) => {
                return InvocationResponse::error(
                    "FILE_READ_FAILED",
                    format!("failed to read notes file '{path}': {error}"),
                );
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                "use either --notes or --notes-file, not both",
            );
        }
    };
    // An omitted option has to stay out of the payload: GitHub rejects an
    // explicit null with `nil is not a string` rather than falling back to its
    // own default.
    let mut body = serde_json::Map::new();
    body.insert("tag_name".to_owned(), Value::String(args.tag));
    body.insert("draft".to_owned(), Value::Bool(args.draft));
    body.insert("prerelease".to_owned(), Value::Bool(args.prerelease));
    for (key, value) in [
        ("target_commitish", args.target),
        ("name", args.title),
        ("body", notes),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), Value::String(value));
        }
    }
    let body = Value::Object(body);
    let path = format!(
        "/repos/{}/{}/releases",
        context.repo.owner, context.repo.repo
    );
    let release = match github_json::<ReleaseResponse>(context, Method::POST, &path, Some(body)) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_release_text(&release, TextFormatter::stdout());
    render::render_success(
        globals,
        &ReleaseOutput {
            command: "github.release.create",
            repository: context.repo.full_name(),
            release,
        },
        text,
    )
}

pub(crate) fn dispatch_workflow(
    context: &GithubContext,
    args: WorkflowRunArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let inputs = match parse_key_values(&args.inputs, "--input") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let body = json!({
        "ref": args.r#ref,
        "inputs": inputs,
    });
    let path = format!(
        "/repos/{}/{}/actions/workflows/{}/dispatches",
        context.repo.owner,
        context.repo.repo,
        urlencoding::encode(&args.workflow)
    );
    if let Err(error) = github_no_content(context, Method::POST, &path, Some(body)) {
        return error;
    }

    let formatter = TextFormatter::stdout();
    let text = format!(
        "{} {} {} {}\n",
        formatter.paint(TextStyle::Success, "dispatched"),
        formatter.paint(TextStyle::Key, &args.workflow),
        formatter.paint(TextStyle::Muted, "on"),
        formatter.paint(TextStyle::Key, &args.r#ref)
    );
    render::render_success(
        globals,
        &WorkflowDispatchOutput {
            command: "github.workflow.run",
            repository: context.repo.full_name(),
            workflow: args.workflow,
            r#ref: args.r#ref,
            input_count: inputs.as_object().map(|value| value.len()).unwrap_or(0),
            dispatched: true,
        },
        text,
    )
}

pub(crate) fn wait_run(
    context: &GithubContext,
    args: WaitRunArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let waited = poll::until_ready(
        Duration::from_secs(args.timeout_secs.max(1)),
        Duration::from_secs(args.interval_secs.max(1)),
        || {
            let run = get_run(context, args.run_id)?;
            Ok(if run.status == "completed" {
                poll::Poll::Ready(run)
            } else {
                poll::Poll::Pending
            })
        },
    );
    let run = match waited {
        Ok(poll::Waited::Ready { value, elapsed }) => (value, elapsed.as_secs()),
        Ok(poll::Waited::TimedOut) => {
            return InvocationResponse::error(
                "GITHUB_RUN_TIMEOUT",
                format!(
                    "workflow run {} did not complete within {} seconds",
                    args.run_id, args.timeout_secs
                ),
            );
        }
        Ok(poll::Waited::Cancelled) => {
            return InvocationResponse::error(
                "CANCELLED",
                format!("workflow run wait {} was cancelled", args.run_id),
            );
        }
        Err(error) => return error,
    };
    let (run, elapsed_secs) = run;
    if args.fail_on_failure && run.conclusion.as_deref() != Some("success") {
        return InvocationResponse::error(
            "GITHUB_RUN_FAILED",
            format!(
                "workflow run {} completed with conclusion {:?}",
                run.id, run.conclusion
            ),
        );
    }
    let text = render_runs_text(std::slice::from_ref(&run), TextFormatter::stdout());
    render::render_success(
        globals,
        &WaitRunOutput {
            command: "github.run.wait",
            repository: context.repo.full_name(),
            run,
            elapsed_secs,
        },
        text,
    )
}

pub(crate) fn run_jobs(
    context: &GithubContext,
    run_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/jobs?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let jobs = match github_json::<JobsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.jobs,
        Err(error) => return error,
    };
    let text = render_jobs_text(&jobs, TextFormatter::stdout());
    render::render_success(
        globals,
        &JobsOutput {
            command: "github.run.jobs",
            repository: context.repo.full_name(),
            run_id,
            job_count: jobs.len(),
            jobs,
        },
        text,
    )
}

pub(crate) fn run_logs(
    context: &GithubContext,
    run_id: u64,
    grep: Option<String>,
    limits: LogLimitArgs,
    globals: &GlobalOptionsWire,
    warnings_only: bool,
) -> InvocationResponse {
    if limits.max_body_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-body-bytes must be >= 1");
    }
    if limits.max_expanded_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-expanded-bytes must be >= 1");
    }
    let (matches, truncated) = match download_run_logs(
        context,
        run_id,
        grep.as_deref(),
        warnings_only,
        globals.limit,
        limits.max_body_bytes,
        limits.max_expanded_bytes,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = matches
        .iter()
        .map(|line| format!("{}:{}: {}", line.file, line.line, line.text))
        .collect::<Vec<_>>()
        .join("\n")
        + if matches.is_empty() { "" } else { "\n" };
    render::render_success(
        globals,
        &LogsOutput {
            command: if warnings_only {
                "github.run.warnings"
            } else {
                "github.run.logs"
            },
            repository: context.repo.full_name(),
            run_id,
            grep,
            match_count: matches.len(),
            truncated,
            matches,
        },
        text,
    )
}

pub(crate) fn run_artifacts(
    context: &GithubContext,
    run_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/artifacts?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let artifacts = match github_json::<ArtifactsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.artifacts,
        Err(error) => return error,
    };
    let text = render_artifacts_text(&artifacts, TextFormatter::stdout());
    render::render_success(
        globals,
        &ArtifactsOutput {
            command: "github.run.artifacts",
            repository: context.repo.full_name(),
            run_id,
            artifact_count: artifacts.len(),
            artifacts,
        },
        text,
    )
}
