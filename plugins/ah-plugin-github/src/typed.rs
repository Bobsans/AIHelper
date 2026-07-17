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
            repo_descriptor(),
            issues_descriptor(),
            issue_view_descriptor(),
            issue_create_descriptor(),
            issue_update_descriptor(),
            issue_close_descriptor(),
            issue_comment_descriptor(),
            issue_comments_descriptor(),
            release_get_descriptor(),
            release_assets_descriptor(),
            release_create_descriptor(),
            workflows_descriptor(),
            workflow_run_descriptor(),
            runs_descriptor(),
            run_get_descriptor(),
            run_wait_descriptor(),
            run_jobs_descriptor(),
            run_logs_descriptor(false),
            run_logs_descriptor(true),
            run_artifacts_descriptor(),
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
        "GitHub command execution was cancelled",
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

fn typed_cli(request: &TypedInvocationRequest) -> Result<GithubCli, CommandError> {
    let arguments = &request.arguments;
    let cwd = PathBuf::from(&request.context.cwd);
    let connection = typed_connection(request);
    let command = match request.command.as_str() {
        "github.repo" => GithubCommand::Repo,
        "github.issues" => GithubCommand::Issues(IssuesArgs {
            state: string_or(arguments, "state", "open"),
            labels: string_array(arguments, "labels"),
            assignee: optional_string(arguments, "assignee"),
            author: optional_string(arguments, "author"),
            since: optional_string(arguments, "since"),
            search: optional_string(arguments, "search"),
        }),
        "github.issue.view" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::View(IssueNumberArgs {
                number: required_u64(arguments, "number", request)?,
            }),
        }),
        "github.issue.create" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::Create(CreateIssueArgs {
                title: required_string(arguments, "title", request)?,
                body: optional_string(arguments, "body"),
                body_file: optional_file(arguments, "body_file", &cwd),
                labels: string_array(arguments, "labels"),
                assignees: string_array(arguments, "assignees"),
            }),
        }),
        "github.issue.update" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::Update(UpdateIssueArgs {
                number: required_u64(arguments, "number", request)?,
                title: optional_string(arguments, "title"),
                body: optional_string(arguments, "body"),
                body_file: optional_file(arguments, "body_file", &cwd),
                state: optional_string(arguments, "state"),
                labels: string_array(arguments, "labels"),
                assignees: string_array(arguments, "assignees"),
            }),
        }),
        "github.issue.close" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::Close(CloseIssueArgs {
                number: required_u64(arguments, "number", request)?,
                comment: optional_string(arguments, "comment"),
                comment_file: optional_file(arguments, "comment_file", &cwd),
            }),
        }),
        "github.issue.comment" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::Comment(CommentIssueArgs {
                number: required_u64(arguments, "number", request)?,
                body: optional_string(arguments, "body"),
                body_file: optional_file(arguments, "body_file", &cwd),
            }),
        }),
        "github.issue.comments" => GithubCommand::Issue(IssueArgs {
            command: IssueCommand::Comments(IssueNumberArgs {
                number: required_u64(arguments, "number", request)?,
            }),
        }),
        "github.release.get" => GithubCommand::Release(ReleaseArgs {
            command: ReleaseCommand::Get(TagArgs {
                tag: required_string(arguments, "tag", request)?,
            }),
        }),
        "github.release.assets" => GithubCommand::Release(ReleaseArgs {
            command: ReleaseCommand::Assets(TagArgs {
                tag: required_string(arguments, "tag", request)?,
            }),
        }),
        "github.release.create" => GithubCommand::Release(ReleaseArgs {
            command: ReleaseCommand::Create(CreateReleaseArgs {
                tag: required_string(arguments, "tag", request)?,
                title: optional_string(arguments, "title"),
                notes: optional_string(arguments, "notes"),
                notes_file: optional_file(arguments, "notes_file", &cwd),
                target: optional_string(arguments, "target"),
                draft: bool_or(arguments, "draft", false),
                prerelease: bool_or(arguments, "prerelease", false),
            }),
        }),
        "github.workflows" => GithubCommand::Workflows,
        "github.workflow.run" => GithubCommand::Workflow(WorkflowArgs {
            command: WorkflowCommand::Run(WorkflowRunArgs {
                workflow: required_string(arguments, "workflow", request)?,
                r#ref: required_string(arguments, "ref", request)?,
                inputs: string_array(arguments, "inputs"),
            }),
        }),
        "github.runs" => GithubCommand::Runs(RunsArgs {
            workflow: optional_string(arguments, "workflow"),
            branch: optional_string(arguments, "branch"),
        }),
        "github.run.get" => GithubCommand::Run(RunArgs {
            command: RunCommand::Get(RunIdArgs {
                run_id: required_u64(arguments, "run_id", request)?,
            }),
        }),
        "github.run.wait" => GithubCommand::Run(RunArgs {
            command: RunCommand::Wait(WaitRunArgs {
                run_id: required_u64(arguments, "run_id", request)?,
                interval_secs: u64_or(arguments, "interval_secs", DEFAULT_WAIT_INTERVAL_SECS),
                timeout_secs: u64_or(arguments, "wait_timeout_secs", DEFAULT_WAIT_TIMEOUT_SECS)
                    .min(remaining_seconds(request)),
                fail_on_failure: bool_or(arguments, "fail_on_failure", false),
            }),
        }),
        "github.run.jobs" => GithubCommand::Run(RunArgs {
            command: RunCommand::Jobs(RunIdArgs {
                run_id: required_u64(arguments, "run_id", request)?,
            }),
        }),
        "github.run.logs" => GithubCommand::Run(RunArgs {
            command: RunCommand::Logs(LogArgs {
                run_id: required_u64(arguments, "run_id", request)?,
                grep: optional_string(arguments, "grep"),
                limits: log_limits(arguments),
            }),
        }),
        "github.run.warnings" => GithubCommand::Run(RunArgs {
            command: RunCommand::Warnings(LogReadArgs {
                run_id: required_u64(arguments, "run_id", request)?,
                limits: log_limits(arguments),
            }),
        }),
        "github.run.artifacts" => GithubCommand::Run(RunArgs {
            command: RunCommand::Artifacts(RunIdArgs {
                run_id: required_u64(arguments, "run_id", request)?,
            }),
        }),
        _ => {
            return Err(command_error(
                request,
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown GitHub command",
                "the command is not present in the GitHub typed catalog",
                false,
            ));
        }
    };
    Ok(GithubCli {
        connection,
        command,
    })
}

fn typed_connection(request: &TypedInvocationRequest) -> GithubConnectionArgs {
    let arguments = &request.arguments;
    GithubConnectionArgs {
        repo: optional_string(arguments, "repo"),
        remote: string_or(arguments, "remote", DEFAULT_REMOTE),
        api_url: string_or(arguments, "api_url", DEFAULT_API_URL),
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

fn log_limits(arguments: &Value) -> LogLimitArgs {
    LogLimitArgs {
        max_body_bytes: usize_or(arguments, "max_body_bytes", DEFAULT_MAX_LOG_BODY_BYTES),
        max_expanded_bytes: usize_or(
            arguments,
            "max_expanded_bytes",
            DEFAULT_MAX_EXPANDED_LOG_BYTES,
        ),
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
                        .unwrap_or("GITHUB_REQUEST_FAILED"),
                ),
            ));
        }
        let code = response
            .error_code
            .unwrap_or_else(|| "GITHUB_REQUEST_FAILED".to_owned());
        let message = response
            .error_message
            .unwrap_or_else(|| "GitHub command failed".to_owned());
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
            "GitHub command returned no structured output",
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
            "GitHub command returned non-object output",
            "typed commands require a JSON object result",
            false,
        )),
        Err(error) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Failed to decode GitHub command output",
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

fn repo_descriptor() -> CommandDescriptor {
    descriptor(
        "github.repo",
        "Inspect GitHub repository",
        "Detect the GitHub repository and return remote plus API metadata.",
        input_schema(Map::new(), Vec::new()),
        repo_output_schema(),
        read_effects(
            "May run Git repository detection and sends a read request to the configured API URL; a supplied token is sent to that host.",
        ),
    )
}

fn issues_descriptor() -> CommandDescriptor {
    let mut properties = Map::new();
    properties.insert(
        "state".to_owned(),
        json!({"type": "string", "enum": ["open", "closed", "all"], "default": "open"}),
    );
    properties.insert("labels".to_owned(), string_array_schema("Issue labels."));
    properties.insert(
        "assignee".to_owned(),
        optional_text_schema("Assignee login."),
    );
    properties.insert("author".to_owned(), optional_text_schema("Author login."));
    properties.insert(
        "since".to_owned(),
        optional_text_schema("ISO date or timestamp."),
    );
    properties.insert(
        "search".to_owned(),
        optional_text_schema("GitHub search query."),
    );
    descriptor(
        "github.issues",
        "List GitHub issues",
        "List or search repository issues with filters and the shared result limit.",
        input_schema(properties, Vec::new()),
        issues_output_schema(),
        read_effects(
            "Reads issue metadata from the configured GitHub API and may expose private repository data.",
        ),
    )
}

fn issue_view_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.view",
        "View GitHub issue",
        "Return one GitHub issue by repository issue number.",
        input_schema(number_properties(), vec!["number"]),
        issue_output_schema("github.issue.view"),
        read_effects("Reads one issue and its metadata from the configured GitHub API."),
    )
}

fn issue_create_descriptor() -> CommandDescriptor {
    let mut properties = text_body_properties("body", "body_file");
    properties.insert("title".to_owned(), required_text_schema("Issue title."));
    properties.insert("labels".to_owned(), string_array_schema("Labels to set."));
    properties.insert(
        "assignees".to_owned(),
        string_array_schema("Assignees to set."),
    );
    descriptor(
        "github.issue.create",
        "Create GitHub issue",
        "Create a repository issue with optional body, labels, and assignees. Use body or body_file, not both.",
        input_schema(properties, vec!["title"]),
        issue_output_schema("github.issue.create"),
        write_effects(
            "Creates a persistent issue and may notify repository participants; body files are read from the execution cwd.",
        ),
    )
}

fn issue_update_descriptor() -> CommandDescriptor {
    let mut properties = number_properties();
    properties.extend(text_body_properties("body", "body_file"));
    properties.insert(
        "title".to_owned(),
        optional_text_schema("Replacement title."),
    );
    properties.insert(
        "state".to_owned(),
        json!({"type": "string", "enum": ["open", "closed"]}),
    );
    properties.insert(
        "labels".to_owned(),
        json!({"type": "array", "minItems": 1, "items": {"type": "string"}}),
    );
    properties.insert(
        "assignees".to_owned(),
        json!({"type": "array", "minItems": 1, "items": {"type": "string"}}),
    );
    descriptor(
        "github.issue.update",
        "Update GitHub issue",
        "Update one or more issue fields. At least one update field is required; use body or body_file, not both.",
        input_schema(properties, vec!["number"]),
        issue_output_schema("github.issue.update"),
        write_effects(
            "Mutates a persistent issue and may change workflow state or notify participants.",
        ),
    )
}

fn issue_close_descriptor() -> CommandDescriptor {
    let mut properties = number_properties();
    properties.extend(text_body_properties("comment", "comment_file"));
    descriptor(
        "github.issue.close",
        "Close GitHub issue",
        "Close an issue, optionally adding a comment first. Use comment or comment_file, not both.",
        input_schema(properties, vec!["number"]),
        issue_output_schema("github.issue.close"),
        write_effects(
            "May create a comment, closes a persistent issue, and may notify participants.",
        ),
    )
}

fn issue_comment_descriptor() -> CommandDescriptor {
    let mut properties = number_properties();
    properties.extend(text_body_properties("body", "body_file"));
    descriptor(
        "github.issue.comment",
        "Comment on GitHub issue",
        "Create a comment on one repository issue. Exactly one of body or body_file is required.",
        input_schema(properties, vec!["number"]),
        issue_comment_output_schema(),
        write_effects("Creates a persistent issue comment and may notify repository participants."),
    )
}

fn issue_comments_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.comments",
        "List GitHub issue comments",
        "List comments for one issue with the shared result limit.",
        input_schema(number_properties(), vec!["number"]),
        issue_comments_output_schema(),
        read_effects("Reads issue comments and author metadata from the configured GitHub API."),
    )
}

fn release_get_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.get",
        "Get GitHub release",
        "Return GitHub release metadata by tag.",
        input_schema(tag_properties(), vec!["tag"]),
        release_output_schema("github.release.get"),
        read_effects("Reads release metadata and asset URLs from the configured GitHub API."),
    )
}

fn release_assets_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.assets",
        "List GitHub release assets",
        "List assets attached to a release tag.",
        input_schema(tag_properties(), vec!["tag"]),
        release_assets_output_schema(),
        read_effects(
            "Reads release asset metadata and download URLs from the configured GitHub API.",
        ),
    )
}

fn release_create_descriptor() -> CommandDescriptor {
    let mut properties = tag_properties();
    properties.extend(text_body_properties("notes", "notes_file"));
    properties.insert("title".to_owned(), optional_text_schema("Release title."));
    properties.insert(
        "target".to_owned(),
        optional_text_schema("Target commit-ish."),
    );
    properties.insert(
        "draft".to_owned(),
        boolean_schema(false, "Create as draft."),
    );
    properties.insert(
        "prerelease".to_owned(),
        boolean_schema(false, "Mark as prerelease."),
    );
    descriptor(
        "github.release.create",
        "Create GitHub release",
        "Create a GitHub release for a tag with optional notes and flags. Use notes or notes_file, not both.",
        input_schema(properties, vec!["tag"]),
        release_output_schema("github.release.create"),
        write_effects(
            "Creates a persistent release and may create or resolve a tag target; release notes files are read from the execution cwd.",
        ),
    )
}

fn workflows_descriptor() -> CommandDescriptor {
    descriptor(
        "github.workflows",
        "List GitHub workflows",
        "List GitHub Actions workflows in the repository.",
        input_schema(Map::new(), Vec::new()),
        workflows_output_schema(),
        read_effects(
            "Reads workflow names, paths, states, and URLs from the configured GitHub API.",
        ),
    )
}

fn workflow_run_descriptor() -> CommandDescriptor {
    let mut properties = Map::new();
    properties.insert(
        "workflow".to_owned(),
        required_text_schema("Workflow id or file name."),
    );
    properties.insert(
        "ref".to_owned(),
        required_text_schema("Git reference to dispatch."),
    );
    properties.insert(
        "inputs".to_owned(),
        json!({
            "type": "array",
            "items": {"type": "string", "pattern": "^[^=]+=.*$"},
            "description": "Workflow inputs encoded as KEY=VALUE."
        }),
    );
    descriptor(
        "github.workflow.run",
        "Dispatch GitHub workflow",
        "Dispatch a GitHub Actions workflow on a reference.",
        input_schema(properties, vec!["workflow", "ref"]),
        workflow_dispatch_output_schema(),
        write_effects(
            "Starts an external workflow that may execute arbitrary repository automation and consume billed resources.",
        ),
    )
}

fn runs_descriptor() -> CommandDescriptor {
    let mut properties = Map::new();
    properties.insert(
        "workflow".to_owned(),
        optional_text_schema("Workflow id or file."),
    );
    properties.insert(
        "branch".to_owned(),
        optional_text_schema("Head branch filter."),
    );
    descriptor(
        "github.runs",
        "List GitHub workflow runs",
        "List workflow runs with optional workflow and branch filters.",
        input_schema(properties, Vec::new()),
        runs_output_schema(),
        read_effects(
            "Reads workflow run status, commit SHA, and URLs from the configured GitHub API.",
        ),
    )
}

fn run_get_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.get",
        "Get GitHub workflow run",
        "Return one workflow run by numeric id.",
        input_schema(run_id_properties(), vec!["run_id"]),
        run_output_schema("github.run.get"),
        read_effects(
            "Reads one workflow run and its commit/status metadata from the configured GitHub API.",
        ),
    )
}

fn run_wait_descriptor() -> CommandDescriptor {
    let mut properties = run_id_properties();
    properties.insert(
        "interval_secs".to_owned(),
        positive_integer_schema(DEFAULT_WAIT_INTERVAL_SECS, "Polling interval."),
    );
    properties.insert(
        "wait_timeout_secs".to_owned(),
        positive_integer_schema(DEFAULT_WAIT_TIMEOUT_SECS, "Maximum wait duration."),
    );
    properties.insert(
        "fail_on_failure".to_owned(),
        boolean_schema(false, "Return an error for a non-success conclusion."),
    );
    descriptor(
        "github.run.wait",
        "Wait for GitHub workflow run",
        "Poll a workflow run until completion, timeout, or cancellation.",
        input_schema(properties, vec!["run_id"]),
        wait_run_output_schema(),
        read_effects(
            "Repeatedly reads external workflow state until completion and may consume API rate limits.",
        ),
    )
}

fn run_jobs_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.jobs",
        "List GitHub workflow jobs",
        "List jobs belonging to one workflow run.",
        input_schema(run_id_properties(), vec!["run_id"]),
        jobs_output_schema(),
        read_effects(
            "Reads workflow job names, status, timestamps, and URLs from the configured GitHub API.",
        ),
    )
}

fn run_logs_descriptor(warnings: bool) -> CommandDescriptor {
    let id = if warnings {
        "github.run.warnings"
    } else {
        "github.run.logs"
    };
    let mut properties = run_id_properties();
    if !warnings {
        properties.insert(
            "grep".to_owned(),
            optional_text_schema("Optional text filter."),
        );
    }
    properties.insert(
        "max_body_bytes".to_owned(),
        positive_integer_schema(
            DEFAULT_MAX_LOG_BODY_BYTES as u64,
            "Maximum compressed response bytes.",
        ),
    );
    properties.insert(
        "max_expanded_bytes".to_owned(),
        positive_integer_schema(
            DEFAULT_MAX_EXPANDED_LOG_BYTES as u64,
            "Maximum expanded archive bytes.",
        ),
    );
    descriptor(
        id,
        if warnings {
            "Extract GitHub run warnings"
        } else {
            "Search GitHub run logs"
        },
        if warnings {
            "Extract warning-like lines from one workflow run log archive."
        } else {
            "Read or filter lines from one workflow run log archive."
        },
        input_schema(properties, vec!["run_id"]),
        logs_output_schema(id),
        read_effects(
            "Downloads and expands workflow logs, which may contain secrets or untrusted build output.",
        ),
    )
}

fn run_artifacts_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.artifacts",
        "List GitHub workflow artifacts",
        "List artifacts produced by one workflow run.",
        input_schema(run_id_properties(), vec!["run_id"]),
        artifacts_output_schema(),
        read_effects(
            "Reads artifact names, sizes, expiry state, and archive URLs from the configured GitHub API.",
        ),
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
        "repo".to_owned(),
        optional_text_schema("Repository override in OWNER/REPO form."),
    );
    properties.insert(
        "remote".to_owned(),
        json!({"type": "string", "minLength": 1, "default": DEFAULT_REMOTE}),
    );
    properties.insert(
        "api_url".to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "default": DEFAULT_API_URL,
            "description": "GitHub-compatible API base URL. A supplied token is sent to this host."
        }),
    );
    properties.insert(
        "token".to_owned(),
        optional_text_schema("Explicit GitHub token; prefer environment-based authentication."),
    );
    properties.insert(
        "use_git_credential".to_owned(),
        boolean_schema(false, "Allow Git credential helper lookup."),
    );
    properties.insert(
        "timeout_secs".to_owned(),
        positive_integer_schema(DEFAULT_TIMEOUT_SECS, "Per-request HTTP timeout."),
    );
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn number_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "number".to_owned(),
        json!({"type": "integer", "minimum": 1, "description": "Issue number."}),
    );
    properties
}

fn run_id_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "run_id".to_owned(),
        json!({"type": "integer", "minimum": 1, "description": "Workflow run id."}),
    );
    properties
}

fn tag_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("tag".to_owned(), required_text_schema("Release tag."));
    properties
}

fn text_body_properties(inline: &str, file: &str) -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(inline.to_owned(), optional_text_schema("Inline text."));
    properties.insert(
        file.to_owned(),
        optional_text_schema("UTF-8 text file resolved against the execution cwd."),
    );
    properties
}

fn required_text_schema(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}

fn optional_text_schema(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn string_array_schema(description: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "description": description
    })
}

fn boolean_schema(default: bool, description: &str) -> Value {
    json!({"type": "boolean", "default": default, "description": description})
}

fn positive_integer_schema(default: u64, description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": default,
        "description": description
    })
}

fn nullable(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn object_schema(properties: Map<String, Value>, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn command_output_schema(
    command: &str,
    mut properties: Map<String, Value>,
    required: &[&str],
) -> Value {
    properties.insert(
        "command".to_owned(),
        json!({"type": "string", "const": command}),
    );
    let mut all_required = vec!["command"];
    all_required.extend_from_slice(required);
    object_schema(properties, &all_required)
}

fn repo_output_schema() -> Value {
    let mut p = Map::new();
    p.insert("repository".to_owned(), json!({"type": "string"}));
    p.insert("owner".to_owned(), json!({"type": "string"}));
    p.insert("name".to_owned(), json!({"type": "string"}));
    p.insert("remote_url".to_owned(), nullable(json!({"type": "string"})));
    p.insert("api_url".to_owned(), json!({"type": "string"}));
    p.insert("html_url".to_owned(), nullable(json!({"type": "string"})));
    p.insert(
        "default_branch".to_owned(),
        nullable(json!({"type": "string"})),
    );
    p.insert("private".to_owned(), nullable(json!({"type": "boolean"})));
    command_output_schema(
        "github.repo",
        p,
        &[
            "repository",
            "owner",
            "name",
            "remote_url",
            "api_url",
            "html_url",
            "default_branch",
            "private",
        ],
    )
}

fn user_schema() -> Value {
    object_schema(
        Map::from_iter([("login".to_owned(), json!({"type": "string"}))]),
        &["login"],
    )
}

fn label_schema() -> Value {
    object_schema(
        Map::from_iter([("name".to_owned(), json!({"type": "string"}))]),
        &["name"],
    )
}

fn issue_schema() -> Value {
    let mut p = Map::new();
    p.insert(
        "number".to_owned(),
        json!({"type": "integer", "minimum": 1}),
    );
    p.insert("title".to_owned(), json!({"type": "string"}));
    p.insert("body".to_owned(), nullable(json!({"type": "string"})));
    p.insert("state".to_owned(), json!({"type": "string"}));
    p.insert("html_url".to_owned(), nullable(json!({"type": "string"})));
    p.insert("user".to_owned(), nullable(user_schema()));
    p.insert(
        "labels".to_owned(),
        json!({"type": "array", "items": label_schema()}),
    );
    p.insert(
        "assignees".to_owned(),
        json!({"type": "array", "items": user_schema()}),
    );
    p.insert(
        "comments".to_owned(),
        nullable(json!({"type": "integer", "minimum": 0})),
    );
    for field in ["created_at", "updated_at", "closed_at"] {
        p.insert(field.to_owned(), nullable(json!({"type": "string"})));
    }
    p.insert(
        "pull_request".to_owned(),
        nullable(json!({"type": "object", "additionalProperties": true})),
    );
    object_schema(
        p,
        &[
            "number",
            "title",
            "body",
            "state",
            "html_url",
            "user",
            "labels",
            "assignees",
            "comments",
            "created_at",
            "updated_at",
            "closed_at",
            "pull_request",
        ],
    )
}

fn issues_output_schema() -> Value {
    let mut p = Map::new();
    p.insert("repository".to_owned(), json!({"type": "string"}));
    p.insert("state".to_owned(), json!({"type": "string"}));
    p.insert(
        "labels".to_owned(),
        json!({"type": "array", "items": {"type": "string"}}),
    );
    for field in ["assignee", "author", "since", "search"] {
        p.insert(field.to_owned(), nullable(json!({"type": "string"})));
    }
    p.insert(
        "issue_count".to_owned(),
        json!({"type": "integer", "minimum": 0}),
    );
    p.insert(
        "issues".to_owned(),
        json!({"type": "array", "items": issue_schema()}),
    );
    command_output_schema(
        "github.issues",
        p,
        &[
            "repository",
            "state",
            "labels",
            "assignee",
            "author",
            "since",
            "search",
            "issue_count",
            "issues",
        ],
    )
}

fn issue_output_schema(command: &str) -> Value {
    command_output_schema(
        command,
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("issue".to_owned(), issue_schema()),
        ]),
        &["repository", "issue"],
    )
}

fn comment_schema() -> Value {
    let mut p = Map::new();
    p.insert("id".to_owned(), json!({"type": "integer", "minimum": 1}));
    p.insert("body".to_owned(), nullable(json!({"type": "string"})));
    p.insert("html_url".to_owned(), nullable(json!({"type": "string"})));
    p.insert("user".to_owned(), nullable(user_schema()));
    p.insert("created_at".to_owned(), nullable(json!({"type": "string"})));
    p.insert("updated_at".to_owned(), nullable(json!({"type": "string"})));
    object_schema(
        p,
        &["id", "body", "html_url", "user", "created_at", "updated_at"],
    )
}

fn issue_comments_output_schema() -> Value {
    command_output_schema(
        "github.issue.comments",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "number".to_owned(),
                json!({"type": "integer", "minimum": 1}),
            ),
            (
                "comment_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "comments".to_owned(),
                json!({"type": "array", "items": comment_schema()}),
            ),
        ]),
        &["repository", "number", "comment_count", "comments"],
    )
}

fn issue_comment_output_schema() -> Value {
    command_output_schema(
        "github.issue.comment",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "number".to_owned(),
                json!({"type": "integer", "minimum": 1}),
            ),
            ("comment".to_owned(), comment_schema()),
        ]),
        &["repository", "number", "comment"],
    )
}

fn release_asset_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("name".to_owned(), json!({"type": "string"})),
            ("size".to_owned(), json!({"type": "integer", "minimum": 0})),
            (
                "browser_download_url".to_owned(),
                nullable(json!({"type": "string"})),
            ),
        ]),
        &["id", "name", "size", "browser_download_url"],
    )
}

fn release_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("tag_name".to_owned(), json!({"type": "string"})),
            ("name".to_owned(), nullable(json!({"type": "string"}))),
            ("draft".to_owned(), json!({"type": "boolean"})),
            ("prerelease".to_owned(), json!({"type": "boolean"})),
            ("html_url".to_owned(), nullable(json!({"type": "string"}))),
            (
                "published_at".to_owned(),
                nullable(json!({"type": "string"})),
            ),
            (
                "assets".to_owned(),
                json!({"type": "array", "items": release_asset_schema()}),
            ),
        ]),
        &[
            "id",
            "tag_name",
            "name",
            "draft",
            "prerelease",
            "html_url",
            "published_at",
            "assets",
        ],
    )
}

fn release_output_schema(command: &str) -> Value {
    command_output_schema(
        command,
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("release".to_owned(), release_schema()),
        ]),
        &["repository", "release"],
    )
}

fn release_assets_output_schema() -> Value {
    command_output_schema(
        "github.release.assets",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("tag".to_owned(), json!({"type": "string"})),
            (
                "asset_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "assets".to_owned(),
                json!({"type": "array", "items": release_asset_schema()}),
            ),
        ]),
        &["repository", "tag", "asset_count", "assets"],
    )
}

fn workflow_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("name".to_owned(), json!({"type": "string"})),
            ("path".to_owned(), json!({"type": "string"})),
            ("state".to_owned(), json!({"type": "string"})),
            ("html_url".to_owned(), nullable(json!({"type": "string"}))),
        ]),
        &["id", "name", "path", "state", "html_url"],
    )
}

fn workflows_output_schema() -> Value {
    command_output_schema(
        "github.workflows",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "workflow_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "workflows".to_owned(),
                json!({"type": "array", "items": workflow_schema()}),
            ),
        ]),
        &["repository", "workflow_count", "workflows"],
    )
}

fn workflow_dispatch_output_schema() -> Value {
    command_output_schema(
        "github.workflow.run",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("workflow".to_owned(), json!({"type": "string"})),
            ("ref".to_owned(), json!({"type": "string"})),
            (
                "input_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            ("dispatched".to_owned(), json!({"type": "boolean"})),
        ]),
        &["repository", "workflow", "ref", "input_count", "dispatched"],
    )
}

fn run_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("name".to_owned(), nullable(json!({"type": "string"}))),
            ("event".to_owned(), json!({"type": "string"})),
            ("status".to_owned(), json!({"type": "string"})),
            ("conclusion".to_owned(), nullable(json!({"type": "string"}))),
            (
                "head_branch".to_owned(),
                nullable(json!({"type": "string"})),
            ),
            ("head_sha".to_owned(), json!({"type": "string"})),
            ("html_url".to_owned(), nullable(json!({"type": "string"}))),
            ("created_at".to_owned(), nullable(json!({"type": "string"}))),
            ("updated_at".to_owned(), nullable(json!({"type": "string"}))),
        ]),
        &[
            "id",
            "name",
            "event",
            "status",
            "conclusion",
            "head_branch",
            "head_sha",
            "html_url",
            "created_at",
            "updated_at",
        ],
    )
}

fn runs_output_schema() -> Value {
    command_output_schema(
        "github.runs",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("workflow".to_owned(), nullable(json!({"type": "string"}))),
            ("branch".to_owned(), nullable(json!({"type": "string"}))),
            (
                "run_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "runs".to_owned(),
                json!({"type": "array", "items": run_schema()}),
            ),
        ]),
        &["repository", "workflow", "branch", "run_count", "runs"],
    )
}

fn run_output_schema(command: &str) -> Value {
    command_output_schema(
        command,
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("run".to_owned(), run_schema()),
        ]),
        &["repository", "run"],
    )
}

fn wait_run_output_schema() -> Value {
    command_output_schema(
        "github.run.wait",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            ("run".to_owned(), run_schema()),
            (
                "elapsed_secs".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
        ]),
        &["repository", "run", "elapsed_secs"],
    )
}

fn job_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("name".to_owned(), json!({"type": "string"})),
            ("status".to_owned(), json!({"type": "string"})),
            ("conclusion".to_owned(), nullable(json!({"type": "string"}))),
            ("html_url".to_owned(), nullable(json!({"type": "string"}))),
            ("started_at".to_owned(), nullable(json!({"type": "string"}))),
            (
                "completed_at".to_owned(),
                nullable(json!({"type": "string"})),
            ),
        ]),
        &[
            "id",
            "name",
            "status",
            "conclusion",
            "html_url",
            "started_at",
            "completed_at",
        ],
    )
}

fn jobs_output_schema() -> Value {
    command_output_schema(
        "github.run.jobs",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "run_id".to_owned(),
                json!({"type": "integer", "minimum": 1}),
            ),
            (
                "job_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "jobs".to_owned(),
                json!({"type": "array", "items": job_schema()}),
            ),
        ]),
        &["repository", "run_id", "job_count", "jobs"],
    )
}

fn log_line_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("file".to_owned(), json!({"type": "string"})),
            ("line".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("text".to_owned(), json!({"type": "string"})),
        ]),
        &["file", "line", "text"],
    )
}

fn logs_output_schema(command: &str) -> Value {
    command_output_schema(
        command,
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "run_id".to_owned(),
                json!({"type": "integer", "minimum": 1}),
            ),
            ("grep".to_owned(), nullable(json!({"type": "string"}))),
            (
                "match_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            ("truncated".to_owned(), json!({"type": "boolean"})),
            (
                "matches".to_owned(),
                json!({"type": "array", "items": log_line_schema()}),
            ),
        ]),
        &[
            "repository",
            "run_id",
            "grep",
            "match_count",
            "truncated",
            "matches",
        ],
    )
}

fn artifact_schema() -> Value {
    object_schema(
        Map::from_iter([
            ("id".to_owned(), json!({"type": "integer", "minimum": 1})),
            ("name".to_owned(), json!({"type": "string"})),
            (
                "size_in_bytes".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            ("expired".to_owned(), json!({"type": "boolean"})),
            (
                "archive_download_url".to_owned(),
                nullable(json!({"type": "string"})),
            ),
        ]),
        &[
            "id",
            "name",
            "size_in_bytes",
            "expired",
            "archive_download_url",
        ],
    )
}

fn artifacts_output_schema() -> Value {
    command_output_schema(
        "github.run.artifacts",
        Map::from_iter([
            ("repository".to_owned(), json!({"type": "string"})),
            (
                "run_id".to_owned(),
                json!({"type": "integer", "minimum": 1}),
            ),
            (
                "artifact_count".to_owned(),
                json!({"type": "integer", "minimum": 0}),
            ),
            (
                "artifacts".to_owned(),
                json!({"type": "array", "items": artifact_schema()}),
            ),
        ]),
        &["repository", "run_id", "artifact_count", "artifacts"],
    )
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn catalog_contains_all_github_commands() {
        let catalog = command_catalog();
        assert_eq!(catalog.commands.len(), 20);
        assert!(catalog.commands.iter().all(|command| {
            command.input_schema["type"] == "object" && command.output_schema["type"] == "object"
        }));
        assert!(catalog.commands.iter().all(|command| {
            let root = command.input_schema.as_object().unwrap();
            ["oneOf", "anyOf", "allOf", "not", "if", "then", "else"]
                .iter()
                .all(|keyword| !root.contains_key(*keyword))
        }));
        assert!(catalog.commands.iter().any(|item| item.id == "github.repo"));
        assert!(
            catalog
                .commands
                .iter()
                .any(|item| item.id == "github.run.warnings")
        );
        assert!(catalog.commands.iter().all(|item| item.effects.open_world));
    }

    #[test]
    fn typed_repo_uses_structured_output_without_network_success() {
        let request = TypedInvocationRequest::new(
            "github.repo",
            json!({
                "repo": "owner/repo",
                "api_url": "http://127.0.0.1:9",
                "timeout_secs": 1
            }),
            ExecutionContextWire::new("github-test", ".", None, 2_000),
        );
        let response = invoke(&request);
        assert!(response.success);
        let data = response.data.unwrap();
        assert_eq!(data["command"], "github.repo");
        assert_eq!(data["repository"], "owner/repo");
    }

    #[test]
    fn cancellation_wait_is_woken() {
        let request_id = "github-cancel-test".to_owned();
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
        let request_id = "github-pre-cancelled";
        assert!(cancel(request_id));
        let request = TypedInvocationRequest::new(
            "github.repo",
            json!({"repo": "owner/repo"}),
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
