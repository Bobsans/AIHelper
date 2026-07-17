use std::{
    cell::RefCell,
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, OnceLock},
    time::Duration,
};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError,
    GlobalOptionsWire, Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use serde_json::{Map, Value, json};

use super::*;

thread_local! {
    static CURRENT_REQUEST_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct CancellationState {
    request_ids: Mutex<HashSet<String>>,
    changed: Condvar,
}

struct RequestCancellationScope {
    request_id: String,
    previous_request_id: Option<String>,
}

impl RequestCancellationScope {
    fn enter(request_id: String) -> Self {
        let previous_request_id =
            CURRENT_REQUEST_ID.with(|current| current.replace(Some(request_id.clone())));
        Self {
            request_id,
            previous_request_id,
        }
    }
}

impl Drop for RequestCancellationScope {
    fn drop(&mut self) {
        CURRENT_REQUEST_ID.with(|current| current.replace(self.previous_request_id.take()));
        cancellation_state()
            .request_ids
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.request_id);
    }
}

pub(super) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        PLUGIN_NAME,
        DOMAIN,
        vec![
            project_descriptor(),
            releases_descriptor(),
            release_get_descriptor(),
            release_create_descriptor(),
            issues_descriptor(),
            issue_view_descriptor(),
            issue_create_descriptor(),
            issue_update_descriptor(),
            issue_close_descriptor(),
            issue_comment_descriptor(),
            issue_comments_descriptor(),
            pipelines_descriptor(),
            pipeline_get_descriptor(),
            pipeline_wait_descriptor(),
            pipeline_jobs_descriptor(),
            job_trace_descriptor(false),
            job_trace_descriptor(true),
        ],
    )
}

pub(super) fn invoke(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let _cancellation_scope = RequestCancellationScope::enter(request.context.request_id.clone());
    if current_request_cancelled() {
        return cancelled_response(request);
    }
    invoke_inner(request)
}

pub(super) fn cancel(request_id: &str) -> bool {
    let state = cancellation_state();
    state
        .request_ids
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(request_id.to_owned());
    state.changed.notify_all();
    true
}

pub(super) fn wait_or_cancel(duration: Duration) -> bool {
    let Some(request_id) = CURRENT_REQUEST_ID.with(|current| current.borrow().clone()) else {
        std::thread::sleep(duration);
        return false;
    };
    let state = cancellation_state();
    let cancelled = state
        .request_ids
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cancelled.contains(&request_id) {
        return true;
    }
    let (cancelled, _) = state
        .changed
        .wait_timeout_while(cancelled, duration, |ids| !ids.contains(&request_id))
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cancelled.contains(&request_id)
}

fn current_request_cancelled() -> bool {
    let Some(request_id) = CURRENT_REQUEST_ID.with(|current| current.borrow().clone()) else {
        return false;
    };
    cancellation_state()
        .request_ids
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&request_id)
}

fn cancelled_response(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    TypedInvocationResponse::error(CommandError::new(
        Some(DOMAIN.to_owned()),
        Some(request.command.clone()),
        "EXECUTION_CANCELLED",
        "GitLab command execution was cancelled",
        format!(
            "request '{}' was cancelled before handler execution",
            request.context.request_id
        ),
        1,
        false,
    ))
}

fn cancellation_state() -> &'static CancellationState {
    static STATE: OnceLock<CancellationState> = OnceLock::new();
    STATE.get_or_init(|| CancellationState {
        request_ids: Mutex::new(HashSet::new()),
        changed: Condvar::new(),
    })
}

fn invoke_inner(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let cli = match typed_cli(request) {
        Ok(cli) => cli,
        Err(error) => return TypedInvocationResponse::error(error),
    };
    let globals = GlobalOptionsWire {
        json: true,
        quiet: false,
        limit: request.context.limit,
    };
    invocation_response(request, execute(cli, &globals))
}

fn typed_cli(request: &TypedInvocationRequest) -> Result<GitlabCli, CommandError> {
    let arguments = &request.arguments;
    let cwd = PathBuf::from(&request.context.cwd);
    let connection = typed_connection(request);
    let command = match request.command.as_str() {
        "gitlab.project" => GitlabCommand::Project,
        "gitlab.releases" => GitlabCommand::Releases,
        "gitlab.release.get" => GitlabCommand::Release(ReleaseArgs {
            command: ReleaseCommand::Get(TagArgs {
                tag: required_string(arguments, "tag", request)?,
            }),
        }),
        "gitlab.release.create" => GitlabCommand::Release(ReleaseArgs {
            command: ReleaseCommand::Create(CreateReleaseArgs {
                tag: required_string(arguments, "tag", request)?,
                name: optional_string(arguments, "name"),
                description: optional_string(arguments, "description"),
                description_file: optional_file(arguments, "description_file", &cwd),
                r#ref: optional_string(arguments, "ref"),
            }),
        }),
        "gitlab.issues" => GitlabCommand::Issues(IssuesArgs {
            state: string_or(arguments, "state", "opened"),
            labels: string_array(arguments, "labels"),
            assignee: optional_string(arguments, "assignee"),
            author: optional_string(arguments, "author"),
            since: optional_string(arguments, "since"),
            search: optional_string(arguments, "search"),
        }),
        "gitlab.issue.view" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::View(IssueViewArgs {
                iid: required_u64(arguments, "iid", request)?,
                full: bool_or(arguments, "full", false),
            }),
        }),
        "gitlab.issue.create" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::Create(CreateIssueArgs {
                title: required_string(arguments, "title", request)?,
                description: optional_string(arguments, "description"),
                description_file: optional_file(arguments, "description_file", &cwd),
                labels: string_array(arguments, "labels"),
                assignee_ids: u64_array(arguments, "assignee_ids"),
            }),
        }),
        "gitlab.issue.update" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::Update(UpdateIssueArgs {
                iid: required_u64(arguments, "iid", request)?,
                title: optional_string(arguments, "title"),
                description: optional_string(arguments, "description"),
                description_file: optional_file(arguments, "description_file", &cwd),
                state: optional_string(arguments, "state"),
                labels: string_array(arguments, "labels"),
                assignee_ids: u64_array(arguments, "assignee_ids"),
            }),
        }),
        "gitlab.issue.close" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::Close(CloseIssueArgs {
                iid: required_u64(arguments, "iid", request)?,
                comment: optional_string(arguments, "comment"),
                comment_file: optional_file(arguments, "comment_file", &cwd),
            }),
        }),
        "gitlab.issue.comment" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::Comment(CommentIssueArgs {
                iid: required_u64(arguments, "iid", request)?,
                body: optional_string(arguments, "body"),
                body_file: optional_file(arguments, "body_file", &cwd),
            }),
        }),
        "gitlab.issue.comments" => GitlabCommand::Issue(IssueArgs {
            command: IssueCommand::Comments(IssueIidArgs {
                iid: required_u64(arguments, "iid", request)?,
            }),
        }),
        "gitlab.pipelines" => GitlabCommand::Pipelines(PipelinesArgs {
            branch: optional_string(arguments, "branch"),
        }),
        "gitlab.pipeline.get" => GitlabCommand::Pipeline(PipelineArgs {
            command: PipelineCommand::Get(PipelineIdArgs {
                pipeline_id: required_u64(arguments, "pipeline_id", request)?,
            }),
        }),
        "gitlab.pipeline.wait" => GitlabCommand::Pipeline(PipelineArgs {
            command: PipelineCommand::Wait(WaitPipelineArgs {
                pipeline_id: required_u64(arguments, "pipeline_id", request)?,
                interval_secs: u64_or(arguments, "interval_secs", DEFAULT_WAIT_INTERVAL_SECS),
                timeout_secs: u64_or(arguments, "wait_timeout_secs", DEFAULT_WAIT_TIMEOUT_SECS)
                    .min(remaining_seconds(request)),
                fail_on_failure: bool_or(arguments, "fail_on_failure", false),
            }),
        }),
        "gitlab.pipeline.jobs" => GitlabCommand::Pipeline(PipelineArgs {
            command: PipelineCommand::Jobs(PipelineIdArgs {
                pipeline_id: required_u64(arguments, "pipeline_id", request)?,
            }),
        }),
        "gitlab.job.trace" => GitlabCommand::Job(JobArgs {
            command: JobCommand::Trace(JobTraceArgs {
                job_id: required_u64(arguments, "job_id", request)?,
                grep: optional_string(arguments, "grep"),
                limits: trace_limits(arguments),
            }),
        }),
        "gitlab.job.warnings" => GitlabCommand::Job(JobArgs {
            command: JobCommand::Warnings(JobTraceReadArgs {
                job_id: required_u64(arguments, "job_id", request)?,
                limits: trace_limits(arguments),
            }),
        }),
        _ => {
            return Err(command_error(
                request,
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown GitLab command",
                "the command is not present in the GitLab typed catalog",
                false,
            ));
        }
    };
    Ok(GitlabCli {
        connection,
        command,
    })
}

fn typed_connection(request: &TypedInvocationRequest) -> GitlabConnectionArgs {
    let arguments = &request.arguments;
    GitlabConnectionArgs {
        project: optional_string(arguments, "project"),
        remote: string_or(arguments, "remote", DEFAULT_REMOTE),
        host: string_or(arguments, "host", DEFAULT_HOST),
        api_url: optional_string(arguments, "api_url"),
        graphql_url: optional_string(arguments, "graphql_url"),
        token: optional_string(arguments, "token"),
        use_git_credential: bool_or(arguments, "use_git_credential", false),
        timeout_secs: u64_or(arguments, "timeout_secs", DEFAULT_TIMEOUT_SECS)
            .min(remaining_seconds(request)),
        cwd: Some(PathBuf::from(&request.context.cwd)),
    }
}

fn remaining_seconds(request: &TypedInvocationRequest) -> u64 {
    request
        .context
        .remaining_timeout_ms
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or(1)
        .max(1)
}

fn trace_limits(arguments: &Value) -> JobTraceLimitArgs {
    JobTraceLimitArgs {
        max_body_bytes: usize_or(arguments, "max_body_bytes", DEFAULT_MAX_TRACE_BODY_BYTES),
    }
}

fn invocation_response(
    request: &TypedInvocationRequest,
    response: InvocationResponse,
) -> TypedInvocationResponse {
    if !response.success {
        if let Some(diagnostic) = response.diagnostic {
            return TypedInvocationResponse::error(CommandError::from_diagnostic(
                diagnostic
                    .with_domain(DOMAIN)
                    .with_operation(request.command.clone()),
                retryable_code(
                    response
                        .error_code
                        .as_deref()
                        .unwrap_or("GITLAB_REQUEST_FAILED"),
                ),
            ));
        }
        let code = response
            .error_code
            .unwrap_or_else(|| "GITLAB_REQUEST_FAILED".to_owned());
        let message = response
            .error_message
            .unwrap_or_else(|| "GitLab command failed".to_owned());
        return TypedInvocationResponse::error(command_error(
            request,
            &code,
            &message,
            &message,
            retryable_code(&code),
        ));
    }
    let Some(raw) = response.message else {
        return TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "GitLab command returned no structured output",
            "the shared command implementation omitted its JSON result",
            false,
        ));
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(data) if data.is_object() => {
            TypedInvocationResponse::success(data, Some(format!("Completed {}.", request.command)))
        }
        Ok(_) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "GitLab command returned non-object output",
            "typed commands require a JSON object result",
            false,
        )),
        Err(error) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Failed to decode GitLab command output",
            error.to_string(),
            false,
        )),
    }
}

fn retryable_code(code: &str) -> bool {
    code.contains("HTTP")
        || code.contains("TIMEOUT")
        || code.contains("RATE")
        || code.contains("SERVER")
}

fn command_error(
    request: &TypedInvocationRequest,
    code: impl Into<String>,
    message: impl Into<String>,
    cause: impl Into<String>,
    retryable: bool,
) -> CommandError {
    CommandError::new(
        Some(DOMAIN.to_owned()),
        Some(request.command.clone()),
        code,
        message,
        cause,
        1,
        retryable,
    )
}

fn required_string(
    arguments: &Value,
    name: &str,
    request: &TypedInvocationRequest,
) -> Result<String, CommandError> {
    optional_string(arguments, name).ok_or_else(|| {
        command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Missing {name}"),
            format!("typed input requires '{name}'"),
            false,
        )
    })
}

fn required_u64(
    arguments: &Value,
    name: &str,
    request: &TypedInvocationRequest,
) -> Result<u64, CommandError> {
    arguments.get(name).and_then(Value::as_u64).ok_or_else(|| {
        command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Missing {name}"),
            format!("typed input requires positive integer '{name}'"),
            false,
        )
    })
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn optional_file(arguments: &Value, name: &str, cwd: &Path) -> Option<String> {
    optional_string(arguments, name).map(|value| {
        let path = Path::new(&value);
        if path.is_absolute() {
            value
        } else {
            cwd.join(path).to_string_lossy().into_owned()
        }
    })
}

fn string_or(arguments: &Value, name: &str, default: &str) -> String {
    optional_string(arguments, name).unwrap_or_else(|| default.to_owned())
}

fn string_array(arguments: &Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn u64_array(arguments: &Value, name: &str) -> Vec<u64> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn bool_or(arguments: &Value, name: &str, default: bool) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn u64_or(arguments: &Value, name: &str, default: u64) -> u64 {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(default)
}

fn usize_or(arguments: &Value, name: &str, default: usize) -> usize {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(default)
}

fn project_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.project",
        "Inspect GitLab project",
        "Detect the GitLab project and return remote plus API metadata.",
        input_schema(Map::new(), Vec::new()),
        top_output(
            "gitlab.project",
            &[
                ("project", string_schema()),
                ("remote_url", nullable(string_schema())),
                ("host", string_schema()),
                ("api_url", string_schema()),
                ("id", nullable(integer_schema())),
                ("path_with_namespace", nullable(string_schema())),
                ("web_url", nullable(string_schema())),
                ("default_branch", nullable(string_schema())),
                ("visibility", nullable(string_schema())),
            ],
        ),
        read_effects(
            "May run Git project detection and sends a read request to the configured API URL; a supplied token is sent to that host.",
        ),
    )
}

fn releases_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.releases",
        "List GitLab releases",
        "List project releases with the shared result limit.",
        input_schema(Map::new(), Vec::new()),
        list_output("gitlab.releases", "release_count", "releases"),
        read_effects("Reads release metadata and assets from the configured GitLab API."),
    )
}

fn release_get_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.release.get",
        "Get GitLab release",
        "Return one GitLab release by tag.",
        input_schema(tag_properties(), vec!["tag"]),
        item_output("gitlab.release.get", "release"),
        read_effects("Reads release metadata and asset links from the configured GitLab API."),
    )
}

fn release_create_descriptor() -> CommandDescriptor {
    let mut properties = tag_properties();
    properties.insert("name".to_owned(), optional_text_schema("Release name."));
    properties.extend(text_file_properties("description", "description_file"));
    properties.insert(
        "ref".to_owned(),
        optional_text_schema("Tag target reference."),
    );
    descriptor(
        "gitlab.release.create",
        "Create GitLab release",
        "Create a project release with optional description and target reference. Use description or description_file, not both.",
        input_schema(properties, vec!["tag"]),
        item_output("gitlab.release.create", "release"),
        write_effects(
            "Creates a persistent release and may create a tag; description files are read from the execution cwd.",
        ),
    )
}

fn issues_descriptor() -> CommandDescriptor {
    let mut properties = Map::new();
    properties.insert(
        "state".to_owned(),
        json!({"type": "string", "enum": ["opened", "closed", "all"], "default": "opened"}),
    );
    properties.insert("labels".to_owned(), string_array_schema());
    for field in ["assignee", "author", "since", "search"] {
        properties.insert(
            field.to_owned(),
            optional_text_schema("Optional issue filter."),
        );
    }
    descriptor(
        "gitlab.issues",
        "List GitLab issues",
        "List project issues with state, label, author, assignee, date, and search filters.",
        input_schema(properties, Vec::new()),
        list_output("gitlab.issues", "issue_count", "issues"),
        read_effects(
            "Reads issue metadata from the configured GitLab API and may expose private project data.",
        ),
    )
}

fn issue_view_descriptor() -> CommandDescriptor {
    let mut properties = iid_properties();
    properties.insert(
        "full".to_owned(),
        boolean_schema(false, "Also load comments and designs."),
    );
    descriptor(
        "gitlab.issue.view",
        "View GitLab issue",
        "Return one issue, optionally with comments, designs, and warnings.",
        input_schema(properties, vec!["iid"]),
        json!({
            "type": "object",
            "oneOf": [
                item_output("gitlab.issue.view", "issue"),
                top_output(
                    "gitlab.issue.view",
                    &[
                        ("project", string_schema()),
                        ("iid", positive_integer_schema()),
                        ("full", json!({"const": true})),
                        ("issue", external_object_schema()),
                        ("comment_count", nonnegative_integer_schema()),
                        ("comments", external_array_schema()),
                        ("design_count", nonnegative_integer_schema()),
                        ("designs", external_array_schema()),
                        ("warnings", json!({"type": "array", "items": string_schema()}))
                    ]
                )
            ]
        }),
        read_effects(
            "Reads issue metadata and, with full=true, comments plus design information through REST and GraphQL.",
        ),
    )
}

fn issue_create_descriptor() -> CommandDescriptor {
    let mut properties = text_file_properties("description", "description_file");
    properties.insert("title".to_owned(), required_text_schema("Issue title."));
    properties.insert("labels".to_owned(), string_array_schema());
    properties.insert("assignee_ids".to_owned(), u64_array_schema());
    descriptor(
        "gitlab.issue.create",
        "Create GitLab issue",
        "Create a project issue with optional description, labels, and assignees. Use description or description_file, not both.",
        input_schema(properties, vec!["title"]),
        item_output("gitlab.issue.create", "issue"),
        write_effects(
            "Creates a persistent issue and may notify project participants; description files are read from the execution cwd.",
        ),
    )
}

fn issue_update_descriptor() -> CommandDescriptor {
    let mut properties = iid_properties();
    properties.extend(text_file_properties("description", "description_file"));
    properties.insert(
        "title".to_owned(),
        optional_text_schema("Replacement title."),
    );
    properties.insert(
        "state".to_owned(),
        json!({"type": "string", "enum": ["opened", "closed"]}),
    );
    properties.insert(
        "labels".to_owned(),
        json!({"type": "array", "minItems": 1, "items": string_schema()}),
    );
    properties.insert(
        "assignee_ids".to_owned(),
        json!({"type": "array", "minItems": 1, "items": positive_integer_schema()}),
    );
    descriptor(
        "gitlab.issue.update",
        "Update GitLab issue",
        "Update one or more fields on a project issue. At least one update field is required; use description or description_file, not both.",
        input_schema(properties, vec!["iid"]),
        item_output("gitlab.issue.update", "issue"),
        write_effects(
            "Mutates a persistent issue and may change workflow state or notify participants.",
        ),
    )
}

fn issue_close_descriptor() -> CommandDescriptor {
    let mut properties = iid_properties();
    properties.extend(text_file_properties("comment", "comment_file"));
    descriptor(
        "gitlab.issue.close",
        "Close GitLab issue",
        "Close an issue, optionally adding a comment first. Use comment or comment_file, not both.",
        input_schema(properties, vec!["iid"]),
        item_output("gitlab.issue.close", "issue"),
        write_effects("May create a note, closes a persistent issue, and may notify participants."),
    )
}

fn issue_comment_descriptor() -> CommandDescriptor {
    let mut properties = iid_properties();
    properties.extend(text_file_properties("body", "body_file"));
    descriptor(
        "gitlab.issue.comment",
        "Comment on GitLab issue",
        "Create a note on one project issue. Exactly one of body or body_file is required.",
        input_schema(properties, vec!["iid"]),
        top_output(
            "gitlab.issue.comment",
            &[
                ("project", string_schema()),
                ("iid", positive_integer_schema()),
                ("comment", external_object_schema()),
            ],
        ),
        write_effects("Creates a persistent issue note and may notify project participants."),
    )
}

fn issue_comments_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.issue.comments",
        "List GitLab issue comments",
        "List notes for one project issue with the shared result limit.",
        input_schema(iid_properties(), vec!["iid"]),
        top_output(
            "gitlab.issue.comments",
            &[
                ("project", string_schema()),
                ("iid", positive_integer_schema()),
                ("comment_count", nonnegative_integer_schema()),
                ("comments", external_array_schema()),
            ],
        ),
        read_effects("Reads issue notes and author metadata from the configured GitLab API."),
    )
}

fn pipelines_descriptor() -> CommandDescriptor {
    let mut properties = Map::new();
    properties.insert("branch".to_owned(), optional_text_schema("Branch filter."));
    descriptor(
        "gitlab.pipelines",
        "List GitLab pipelines",
        "List project pipelines with an optional branch filter.",
        input_schema(properties, Vec::new()),
        list_output("gitlab.pipelines", "pipeline_count", "pipelines"),
        read_effects(
            "Reads pipeline status, commit SHA, references, and URLs from the configured GitLab API.",
        ),
    )
}

fn pipeline_get_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipeline.get",
        "Get GitLab pipeline",
        "Return one project pipeline by id.",
        input_schema(pipeline_id_properties(), vec!["pipeline_id"]),
        item_output("gitlab.pipeline.get", "pipeline"),
        read_effects(
            "Reads one pipeline and its commit/status metadata from the configured GitLab API.",
        ),
    )
}

fn pipeline_wait_descriptor() -> CommandDescriptor {
    let mut properties = pipeline_id_properties();
    properties.insert(
        "interval_secs".to_owned(),
        integer_with_default(DEFAULT_WAIT_INTERVAL_SECS, "Polling interval."),
    );
    properties.insert(
        "wait_timeout_secs".to_owned(),
        integer_with_default(DEFAULT_WAIT_TIMEOUT_SECS, "Maximum wait duration."),
    );
    properties.insert(
        "fail_on_failure".to_owned(),
        boolean_schema(false, "Return an error for a non-success status."),
    );
    descriptor(
        "gitlab.pipeline.wait",
        "Wait for GitLab pipeline",
        "Poll a pipeline until completion, timeout, or cancellation.",
        input_schema(properties, vec!["pipeline_id"]),
        top_output(
            "gitlab.pipeline.wait",
            &[
                ("project", string_schema()),
                ("pipeline", external_object_schema()),
                ("elapsed_secs", nonnegative_integer_schema()),
            ],
        ),
        read_effects("Repeatedly reads external pipeline state and may consume API rate limits."),
    )
}

fn pipeline_jobs_descriptor() -> CommandDescriptor {
    descriptor(
        "gitlab.pipeline.jobs",
        "List GitLab pipeline jobs",
        "List jobs belonging to one project pipeline.",
        input_schema(pipeline_id_properties(), vec!["pipeline_id"]),
        top_output(
            "gitlab.pipeline.jobs",
            &[
                ("project", string_schema()),
                ("pipeline_id", positive_integer_schema()),
                ("job_count", nonnegative_integer_schema()),
                ("jobs", external_array_schema()),
            ],
        ),
        read_effects(
            "Reads job names, stages, statuses, timestamps, and URLs from the configured GitLab API.",
        ),
    )
}

fn job_trace_descriptor(warnings: bool) -> CommandDescriptor {
    let id = if warnings {
        "gitlab.job.warnings"
    } else {
        "gitlab.job.trace"
    };
    let mut properties = job_id_properties();
    if !warnings {
        properties.insert(
            "grep".to_owned(),
            optional_text_schema("Optional text filter."),
        );
    }
    properties.insert(
        "max_body_bytes".to_owned(),
        integer_with_default(
            DEFAULT_MAX_TRACE_BODY_BYTES as u64,
            "Maximum trace response bytes.",
        ),
    );
    descriptor(
        id,
        if warnings {
            "Extract GitLab job warnings"
        } else {
            "Read GitLab job trace"
        },
        if warnings {
            "Extract warning-like lines from one job trace."
        } else {
            "Read or filter one job trace."
        },
        input_schema(properties, vec!["job_id"]),
        top_output(
            id,
            &[
                ("project", string_schema()),
                ("job_id", positive_integer_schema()),
                ("grep", nullable(string_schema())),
                ("match_count", nonnegative_integer_schema()),
                ("truncated", json!({"type": "boolean"})),
                (
                    "matches",
                    json!({
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "line": positive_integer_schema(),
                                "text": string_schema()
                            },
                            "required": ["line", "text"],
                            "additionalProperties": false
                        }
                    }),
                ),
            ],
        ),
        read_effects("Downloads a job trace that may contain secrets or untrusted build output."),
    )
}

fn descriptor(
    id: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
    effects: CommandEffects,
) -> CommandDescriptor {
    CommandDescriptor::new(id, title, description, input_schema, output_schema, effects)
}

fn read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        true,
        vec![
            CommandEffect::NetworkRead,
            CommandEffect::ExternalRead,
            CommandEffect::ConfigurationRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::Medium,
        impact,
        Reversibility::Yes,
    )
}

fn write_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        false,
        false,
        false,
        true,
        vec![
            CommandEffect::NetworkWrite,
            CommandEffect::ExternalWrite,
            CommandEffect::ConfigurationRead,
            CommandEffect::FilesystemRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::High,
        impact,
        Reversibility::Unknown,
    )
}

fn input_schema(mut properties: Map<String, Value>, required: Vec<&str>) -> Value {
    properties.insert(
        "project".to_owned(),
        optional_text_schema("Project override as path or numeric id."),
    );
    properties.insert(
        "remote".to_owned(),
        json!({"type": "string", "minLength": 1, "default": DEFAULT_REMOTE}),
    );
    properties.insert(
        "host".to_owned(),
        json!({"type": "string", "minLength": 1, "default": DEFAULT_HOST}),
    );
    properties.insert(
        "api_url".to_owned(),
        optional_text_schema("REST API base URL. A supplied token is sent to this host."),
    );
    properties.insert(
        "graphql_url".to_owned(),
        optional_text_schema("GraphQL API URL used for issue designs."),
    );
    properties.insert(
        "token".to_owned(),
        optional_text_schema("Explicit GitLab token; prefer environment-based authentication."),
    );
    properties.insert(
        "use_git_credential".to_owned(),
        boolean_schema(false, "Allow Git credential helper lookup."),
    );
    properties.insert(
        "timeout_secs".to_owned(),
        integer_with_default(DEFAULT_TIMEOUT_SECS, "Per-request HTTP timeout."),
    );
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn iid_properties() -> Map<String, Value> {
    Map::from_iter([(
        "iid".to_owned(),
        json!({"type": "integer", "minimum": 1, "description": "Project issue iid."}),
    )])
}

fn tag_properties() -> Map<String, Value> {
    Map::from_iter([("tag".to_owned(), required_text_schema("Release tag."))])
}

fn pipeline_id_properties() -> Map<String, Value> {
    Map::from_iter([(
        "pipeline_id".to_owned(),
        json!({"type": "integer", "minimum": 1}),
    )])
}

fn job_id_properties() -> Map<String, Value> {
    Map::from_iter([(
        "job_id".to_owned(),
        json!({"type": "integer", "minimum": 1}),
    )])
}

fn text_file_properties(inline: &str, file: &str) -> Map<String, Value> {
    Map::from_iter([
        (inline.to_owned(), optional_text_schema("Inline text.")),
        (
            file.to_owned(),
            optional_text_schema("UTF-8 text file resolved against the execution cwd."),
        ),
    ])
}

fn required_text_schema(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}

fn optional_text_schema(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn string_array_schema() -> Value {
    json!({"type": "array", "items": string_schema()})
}

fn u64_array_schema() -> Value {
    json!({"type": "array", "items": positive_integer_schema()})
}

fn boolean_schema(default: bool, description: &str) -> Value {
    json!({"type": "boolean", "default": default, "description": description})
}

fn integer_with_default(default: u64, description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": default,
        "description": description
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn integer_schema() -> Value {
    json!({"type": "integer"})
}

fn positive_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 1})
}

fn nonnegative_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 0})
}

fn nullable(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn external_object_schema() -> Value {
    json!({"type": "object", "additionalProperties": true})
}

fn external_array_schema() -> Value {
    json!({"type": "array", "items": external_object_schema()})
}

fn top_output(command: &str, fields: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "command".to_owned(),
        json!({"type": "string", "const": command}),
    );
    let mut required = vec!["command"];
    for (name, schema) in fields {
        properties.insert((*name).to_owned(), schema.clone());
        required.push(name);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn list_output(command: &str, count: &str, items: &str) -> Value {
    top_output(
        command,
        &[
            ("project", string_schema()),
            (count, nonnegative_integer_schema()),
            (items, external_array_schema()),
        ],
    )
}

fn item_output(command: &str, item: &str) -> Value {
    top_output(
        command,
        &[
            ("project", string_schema()),
            (item, external_object_schema()),
        ],
    )
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn catalog_contains_all_gitlab_commands() {
        let catalog = command_catalog();
        assert_eq!(catalog.commands.len(), 17);
        assert!(catalog.commands.iter().all(|command| {
            command.input_schema["type"] == "object" && command.output_schema["type"] == "object"
        }));
        assert!(catalog.commands.iter().all(|command| {
            let root = command.input_schema.as_object().unwrap();
            ["oneOf", "anyOf", "allOf", "not", "if", "then", "else"]
                .iter()
                .all(|keyword| !root.contains_key(*keyword))
        }));
        assert!(
            catalog
                .commands
                .iter()
                .any(|item| item.id == "gitlab.issue.view")
        );
        assert!(
            catalog
                .commands
                .iter()
                .any(|item| item.id == "gitlab.job.warnings")
        );
    }

    #[test]
    fn typed_project_uses_structured_output_without_network_success() {
        let request = TypedInvocationRequest::new(
            "gitlab.project",
            json!({
                "project": "group/project",
                "host": "http://127.0.0.1:9",
                "timeout_secs": 1
            }),
            ExecutionContextWire::new("gitlab-test", ".", None, 2_000),
        );
        let response = invoke(&request);
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "gitlab.project");
        assert_eq!(data["project"], "group/project");
    }

    #[test]
    fn cancellation_wait_is_woken() {
        let request_id = "gitlab-cancel-test".to_owned();
        CURRENT_REQUEST_ID.with(|current| {
            current.replace(Some(request_id.clone()));
        });
        assert!(cancel(&request_id));
        assert!(wait_or_cancel(Duration::from_secs(1)));
        CURRENT_REQUEST_ID.with(|current| {
            current.replace(None);
        });
        cancellation_state()
            .request_ids
            .lock()
            .unwrap()
            .remove(&request_id);
    }

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "gitlab-pre-cancelled";
        assert!(cancel(request_id));
        let request = TypedInvocationRequest::new(
            "gitlab.project",
            json!({"project": "group/project"}),
            ExecutionContextWire::new(request_id, ".", None, 1_000),
        );

        let response = invoke(&request);

        assert!(!response.success);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("EXECUTION_CANCELLED")
        );
        assert!(
            !cancellation_state()
                .request_ids
                .lock()
                .unwrap()
                .contains(request_id)
        );
    }
}
