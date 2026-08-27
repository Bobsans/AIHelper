//! One function per command, each turning parsed arguments into a response.
//!
//! This is where the two forge plugins genuinely differ - GitLab streams a job
//! trace, has issue designs behind a GraphQL query, and calls a pipeline what
//! GitHub calls a run - so it is the file that has no counterpart on the
//! GitHub side.

use super::*;

pub(crate) fn execute_project(
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!("/projects/{}", context.project.encoded());
    let project_response = gitlab_json::<GitlabProjectResponse>(context, Method::GET, &path, None);
    let (id, path_with_namespace, web_url, default_branch, visibility) = match project_response {
        Ok(value) => (
            value.id,
            value.path_with_namespace,
            value.web_url,
            value.default_branch,
            value.visibility,
        ),
        Err(_) => (None, None, None, None, None),
    };

    let output = ProjectOutput {
        command: "gitlab.project",
        project: context.project.value.clone(),
        remote_url: context.remote_url.clone(),
        host: context.host.clone(),
        api_url: context.api_url.clone(),
        id,
        path_with_namespace,
        web_url,
        default_branch,
        visibility,
    };

    render::render_success(
        globals,
        &output,
        format!(
            "{}\n",
            TextFormatter::stdout().paint(TextStyle::Key, &output.project)
        ),
    )
}

pub(crate) fn execute_issues(
    args: IssuesArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = gitlab_issues_list_path(context, &args, per_page);
    let issues = match gitlab_json::<Vec<IssueResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssuesOutput {
            command: "gitlab.issues",
            project: context.project.value.clone(),
            state: args.state,
            labels: args.labels,
            assignee: args.assignee,
            author: args.author,
            since: args.since,
            search: args.search,
            issue_count: issues.len(),
            issues: issues.clone(),
        },
        render_issues_text(&issues, TextFormatter::stdout()),
    )
}

pub(crate) fn execute_issue(
    args: IssueArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        IssueCommand::View(args) => {
            let issue = match get_issue(context, args.iid) {
                Ok(value) => value,
                Err(error) => return error,
            };
            if args.full {
                return issue_view_full(context, globals, args, issue);
            }
            render::render_success(
                globals,
                &IssueOutput {
                    command: "gitlab.issue.view",
                    project: context.project.value.clone(),
                    issue: issue.clone(),
                },
                render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
            )
        }
        IssueCommand::Create(args) => create_issue(context, args, globals),
        IssueCommand::Update(args) => update_issue(context, args, globals),
        IssueCommand::Close(args) => close_issue(context, args, globals),
        IssueCommand::Comment(args) => comment_issue(context, args, globals),
        IssueCommand::Comments(args) => issue_comments(context, args.iid, globals),
    }
}

pub(crate) fn issue_view_full(
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
    args: IssueViewArgs,
    issue: IssueResponse,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(100).clamp(1, 100);
    let comments = match get_issue_comments(context, args.iid, per_page) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let (designs, warnings) = match get_issue_designs_best_effort(context, args.iid, per_page) {
        Ok(value) => (value, Vec::new()),
        Err(warning) => (Vec::new(), vec![warning]),
    };
    render::render_success(
        globals,
        &IssueFullOutput {
            command: "gitlab.issue.view",
            project: context.project.value.clone(),
            iid: args.iid,
            full: true,
            issue: issue.clone(),
            comment_count: comments.len(),
            comments: comments.clone(),
            design_count: designs.len(),
            designs: designs.clone(),
            warnings: warnings.clone(),
        },
        render_issue_full_text(
            &issue,
            &comments,
            &designs,
            &warnings,
            TextFormatter::stdout(),
        ),
    )
}

pub(crate) fn create_issue(
    context: &GitlabContext,
    args: CreateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description = match text::optional(args.description, args.description_file, "description") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), Value::String(args.title));
    if let Some(description) = description {
        body.insert("description".to_owned(), Value::String(description));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), Value::String(args.labels.join(",")));
    }
    if !args.assignee_ids.is_empty() {
        body.insert("assignee_ids".to_owned(), json!(args.assignee_ids));
    }
    let path = format!("/projects/{}/issues", context.project.encoded());
    let issue =
        match gitlab_json::<IssueResponse>(context, Method::POST, &path, Some(Value::Object(body)))
        {
            Ok(value) => value,
            Err(error) => return error,
        };
    render::render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.create",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn update_issue(
    context: &GitlabContext,
    args: UpdateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description = match text::optional(args.description, args.description_file, "description") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    if let Some(title) = args.title {
        body.insert("title".to_owned(), Value::String(title));
    }
    if let Some(description) = description {
        body.insert("description".to_owned(), Value::String(description));
    }
    if let Some(state) = args.state {
        body.insert(
            "state_event".to_owned(),
            Value::String(gitlab_state_event(&state).to_owned()),
        );
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), Value::String(args.labels.join(",")));
    }
    if !args.assignee_ids.is_empty() {
        body.insert("assignee_ids".to_owned(), json!(args.assignee_ids));
    }
    if body.is_empty() {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            "issue update requires at least one field",
        );
    }
    let path = format!(
        "/projects/{}/issues/{}",
        context.project.encoded(),
        args.iid
    );
    let issue = match gitlab_json::<IssueResponse>(
        context,
        Method::PUT,
        &path,
        Some(Value::Object(body)),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.update",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn close_issue(
    context: &GitlabContext,
    args: CloseIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let comment = match text::optional(args.comment, args.comment_file, "comment") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(comment) = comment
        && let Err(error) = create_issue_note(context, args.iid, comment)
    {
        return error;
    }
    let path = format!(
        "/projects/{}/issues/{}",
        context.project.encoded(),
        args.iid
    );
    let issue = match gitlab_json::<IssueResponse>(
        context,
        Method::PUT,
        &path,
        Some(json!({ "state_event": "close" })),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.close",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

pub(crate) fn comment_issue(
    context: &GitlabContext,
    args: CommentIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body = match text::required(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let comment = match create_issue_note(context, args.iid, body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueNoteOutput {
            command: "gitlab.issue.comment",
            project: context.project.value.clone(),
            iid: args.iid,
            comment: comment.clone(),
        },
        render_comments_text(std::slice::from_ref(&comment), TextFormatter::stdout()),
    )
}

pub(crate) fn issue_comments(
    context: &GitlabContext,
    iid: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let comments = match get_issue_comments(context, iid, per_page) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueNotesOutput {
            command: "gitlab.issue.comments",
            project: context.project.value.clone(),
            iid,
            comment_count: comments.len(),
            comments: comments.clone(),
        },
        render_comments_text(&comments, TextFormatter::stdout()),
    )
}

pub(crate) fn execute_releases(
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = format!(
        "/projects/{}/releases?per_page={per_page}",
        context.project.encoded()
    );
    let releases = match gitlab_json::<Vec<ReleaseResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_releases_text(&releases, TextFormatter::stdout());
    render::render_success(
        globals,
        &ReleasesOutput {
            command: "gitlab.releases",
            project: context.project.value.clone(),
            release_count: releases.len(),
            releases,
        },
        text,
    )
}

pub(crate) fn execute_release(
    args: ReleaseArgs,
    context: &GitlabContext,
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
                    command: "gitlab.release.get",
                    project: context.project.value.clone(),
                    release,
                },
                text,
            )
        }
        ReleaseCommand::Create(args) => create_release(context, args, globals),
    }
}

pub(crate) fn execute_pipelines(
    args: PipelinesArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(10).clamp(1, 100);
    let ref_query = args
        .branch
        .as_ref()
        .map(|branch| format!("&ref={}", urlencoding::encode(branch)))
        .unwrap_or_default();
    let path = format!(
        "/projects/{}/pipelines?per_page={per_page}{ref_query}",
        context.project.encoded()
    );
    let pipelines = match gitlab_json::<Vec<PipelineResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_pipelines_text(&pipelines, TextFormatter::stdout());
    render::render_success(
        globals,
        &PipelinesOutput {
            command: "gitlab.pipelines",
            project: context.project.value.clone(),
            branch: args.branch,
            pipeline_count: pipelines.len(),
            pipelines,
        },
        text,
    )
}

pub(crate) fn execute_pipeline(
    args: PipelineArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        PipelineCommand::Get(args) => {
            let pipeline = match get_pipeline(context, args.pipeline_id) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text =
                render_pipelines_text(std::slice::from_ref(&pipeline), TextFormatter::stdout());
            render::render_success(
                globals,
                &PipelineOutput {
                    command: "gitlab.pipeline.get",
                    project: context.project.value.clone(),
                    pipeline,
                },
                text,
            )
        }
        PipelineCommand::Wait(args) => wait_pipeline(context, args, globals),
        PipelineCommand::Jobs(args) => pipeline_jobs(context, args.pipeline_id, globals),
    }
}

pub(crate) fn execute_job(
    args: JobArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        JobCommand::Trace(args) => {
            job_trace(context, args.job_id, args.grep, args.limits, globals, false)
        }
        JobCommand::Warnings(args) => {
            job_trace(context, args.job_id, None, args.limits, globals, true)
        }
    }
}

pub(crate) fn create_release(
    context: &GitlabContext,
    args: CreateReleaseArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description = match (args.description, args.description_file) {
        (Some(description), None) => Some(description),
        (None, Some(path)) => match fs::read_to_string(&path) {
            Ok(value) => Some(value),
            Err(error) => {
                return InvocationResponse::error(
                    "FILE_READ_FAILED",
                    format!("failed to read description file '{path}': {error}"),
                );
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                "use either --description or --description-file, not both",
            );
        }
    };
    // An omitted option has to stay out of the payload rather than be sent as
    // an explicit null, which GitLab reads as "clear this field".
    let mut body = serde_json::Map::new();
    body.insert("tag_name".to_owned(), Value::String(args.tag));
    for (key, value) in [
        ("name", args.name),
        ("description", description),
        ("ref", args.r#ref),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), Value::String(value));
        }
    }
    let body = Value::Object(body);
    let path = format!("/projects/{}/releases", context.project.encoded());
    let release = match gitlab_json::<ReleaseResponse>(context, Method::POST, &path, Some(body)) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_release_text(&release, TextFormatter::stdout());
    render::render_success(
        globals,
        &ReleaseOutput {
            command: "gitlab.release.create",
            project: context.project.value.clone(),
            release,
        },
        text,
    )
}

pub(crate) fn wait_pipeline(
    context: &GitlabContext,
    args: WaitPipelineArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let waited = poll::until_ready(
        Duration::from_secs(args.timeout_secs.max(1)),
        Duration::from_secs(args.interval_secs.max(1)),
        || {
            let pipeline = get_pipeline(context, args.pipeline_id)?;
            Ok(if is_pipeline_terminal(&pipeline.status) {
                poll::Poll::Ready(pipeline)
            } else {
                poll::Poll::Pending
            })
        },
    );
    let (pipeline, elapsed_secs) = match waited {
        Ok(poll::Waited::Ready { value, elapsed }) => (value, elapsed.as_secs()),
        Ok(poll::Waited::TimedOut) => return pipeline_timeout_response(&args),
        Ok(poll::Waited::Cancelled) => {
            return InvocationResponse::error(
                "CANCELLED",
                format!("pipeline wait {} was cancelled", args.pipeline_id),
            );
        }
        Err(error) => return error,
    };
    if args.fail_on_failure && pipeline.status != "success" {
        return InvocationResponse::error(
            "GITLAB_PIPELINE_FAILED",
            format!(
                "pipeline {} completed with status {}",
                pipeline.id, pipeline.status
            ),
        );
    }
    let text = render_pipelines_text(std::slice::from_ref(&pipeline), TextFormatter::stdout());
    render::render_success(
        globals,
        &WaitPipelineOutput {
            command: "gitlab.pipeline.wait",
            project: context.project.value.clone(),
            pipeline,
            elapsed_secs,
        },
        text,
    )
}

pub(crate) fn pipeline_timeout_response(args: &WaitPipelineArgs) -> InvocationResponse {
    InvocationResponse::error(
        "GITLAB_PIPELINE_TIMEOUT",
        format!(
            "pipeline {} did not complete within {} seconds",
            args.pipeline_id, args.timeout_secs
        ),
    )
}

pub(crate) fn pipeline_jobs(
    context: &GitlabContext,
    pipeline_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/projects/{}/pipelines/{pipeline_id}/jobs?per_page=100",
        context.project.encoded()
    );
    let jobs = match gitlab_json::<Vec<JobResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_jobs_text(&jobs, TextFormatter::stdout());
    render::render_success(
        globals,
        &JobsOutput {
            command: "gitlab.pipeline.jobs",
            project: context.project.value.clone(),
            pipeline_id,
            job_count: jobs.len(),
            jobs,
        },
        text,
    )
}

pub(crate) fn job_trace(
    context: &GitlabContext,
    job_id: u64,
    grep: Option<String>,
    limits: JobTraceLimitArgs,
    globals: &GlobalOptionsWire,
    warnings_only: bool,
) -> InvocationResponse {
    if limits.max_body_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-body-bytes must be >= 1");
    }
    let (matches, truncated) = match collect_job_trace(
        context,
        job_id,
        grep.as_deref(),
        warnings_only,
        globals.limit,
        limits.max_body_bytes,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = matches
        .iter()
        .map(|line| format!("{}: {}", line.line, line.text))
        .collect::<Vec<_>>()
        .join("\n")
        + if matches.is_empty() { "" } else { "\n" };
    render::render_success(
        globals,
        &TraceOutput {
            command: if warnings_only {
                "gitlab.job.warnings"
            } else {
                "gitlab.job.trace"
            },
            project: context.project.value.clone(),
            job_id,
            grep,
            match_count: matches.len(),
            truncated,
            matches,
        },
        text,
    )
}

pub(crate) fn gitlab_state_event(state: &str) -> &'static str {
    match state {
        "closed" => "close",
        _ => "reopen",
    }
}

pub(crate) fn is_pipeline_terminal(status: &str) -> bool {
    matches!(
        status,
        "success" | "failed" | "canceled" | "skipped" | "manual"
    )
}
