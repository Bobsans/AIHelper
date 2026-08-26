use std::path::{Path, PathBuf};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError,
    GlobalOptionsWire, Reversibility, RiskLevel, SecretSlot, TypedInvocationRequest,
    TypedInvocationResponse, cancellation,
    schema::{input_schema_for, output_schema_for},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::*;

// Every command takes the same connection block plus its own arguments.
// `deny_unknown_fields` cannot be derived through `flatten`, so the closed-object
// rule is stated for the schema and the runtime rejects unknown properties
// against it before dispatch.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
struct Wire<T> {
    #[serde(flatten)]
    connection: GithubConnectionArgs,
    #[serde(flatten)]
    command: T,
}

// A command whose only arguments are the connection block.
#[derive(Debug, Deserialize, JsonSchema)]
struct NoArgs {}

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the type disagree.
fn decode<T: serde::de::DeserializeOwned>(
    request: &TypedInvocationRequest,
) -> Result<Wire<T>, CommandError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        command_error(
            request,
            "INVALID_ARGUMENT",
            "GitHub command arguments are invalid",
            error.to_string(),
            false,
        )
    })
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
    let _cancellation_scope = cancellation::RequestScope::enter(&request.context.request_id);
    if cancellation::is_cancelled() {
        return cancelled_response(request);
    }
    invoke_inner(request)
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

fn invoke_inner(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let cli = match typed_cli(request) {
        Ok(cli) => cli,
        Err(error) => return TypedInvocationResponse::error(error),
    };
    let globals = GlobalOptionsWire {
        json: true,
        quiet: false,
        limit: request.context.limit,
        cwd: None,
    };
    invocation_response(request, execute(cli, &globals))
}

fn typed_cli(request: &TypedInvocationRequest) -> Result<GithubCli, CommandError> {
    let cwd = PathBuf::from(&request.context.cwd);

    macro_rules! decoded {
        ($args:ty) => {{
            let wire: Wire<$args> = decode(request)?;
            (wire.connection, wire.command)
        }};
    }

    let (connection, command) = match request.command.as_str() {
        "github.repo" => (decoded!(NoArgs).0, GithubCommand::Repo),
        "github.workflows" => (decoded!(NoArgs).0, GithubCommand::Workflows),
        "github.issues" => {
            let (connection, args) = decoded!(IssuesArgs);
            (connection, GithubCommand::Issues(args))
        }
        "github.issue.view" => {
            let (connection, args) = decoded!(IssueNumberArgs);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::View(args),
                }),
            )
        }
        "github.issue.create" => {
            let (connection, mut args) = decoded!(CreateIssueArgs);
            args.body_file = resolve_file(args.body_file, &cwd);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::Create(args),
                }),
            )
        }
        "github.issue.update" => {
            let (connection, mut args) = decoded!(UpdateIssueArgs);
            args.body_file = resolve_file(args.body_file, &cwd);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::Update(args),
                }),
            )
        }
        "github.issue.close" => {
            let (connection, mut args) = decoded!(CloseIssueArgs);
            args.comment_file = resolve_file(args.comment_file, &cwd);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::Close(args),
                }),
            )
        }
        "github.issue.comment" => {
            let (connection, mut args) = decoded!(CommentIssueArgs);
            args.body_file = resolve_file(args.body_file, &cwd);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::Comment(args),
                }),
            )
        }
        "github.issue.comments" => {
            let (connection, args) = decoded!(IssueNumberArgs);
            (
                connection,
                GithubCommand::Issue(IssueArgs {
                    command: IssueCommand::Comments(args),
                }),
            )
        }
        "github.release.get" => {
            let (connection, args) = decoded!(TagArgs);
            (
                connection,
                GithubCommand::Release(ReleaseArgs {
                    command: ReleaseCommand::Get(args),
                }),
            )
        }
        "github.release.assets" => {
            let (connection, args) = decoded!(TagArgs);
            (
                connection,
                GithubCommand::Release(ReleaseArgs {
                    command: ReleaseCommand::Assets(args),
                }),
            )
        }
        "github.release.create" => {
            let (connection, mut args) = decoded!(CreateReleaseArgs);
            args.notes_file = resolve_file(args.notes_file, &cwd);
            (
                connection,
                GithubCommand::Release(ReleaseArgs {
                    command: ReleaseCommand::Create(args),
                }),
            )
        }
        "github.workflow.run" => {
            let (connection, args) = decoded!(WorkflowRunArgs);
            (
                connection,
                GithubCommand::Workflow(WorkflowArgs {
                    command: WorkflowCommand::Run(args),
                }),
            )
        }
        "github.runs" => {
            let (connection, args) = decoded!(RunsArgs);
            (connection, GithubCommand::Runs(args))
        }
        "github.run.get" => {
            let (connection, args) = decoded!(RunIdArgs);
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Get(args),
                }),
            )
        }
        "github.run.wait" => {
            let (connection, mut args) = decoded!(WaitRunArgs);
            args.timeout_secs = args.timeout_secs.min(remaining_seconds(request));
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Wait(args),
                }),
            )
        }
        "github.run.jobs" => {
            let (connection, args) = decoded!(RunIdArgs);
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Jobs(args),
                }),
            )
        }
        "github.run.logs" => {
            let (connection, args) = decoded!(LogArgs);
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Logs(args),
                }),
            )
        }
        "github.run.warnings" => {
            let (connection, args) = decoded!(LogReadArgs);
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Warnings(args),
                }),
            )
        }
        "github.run.artifacts" => {
            let (connection, args) = decoded!(RunIdArgs);
            (
                connection,
                GithubCommand::Run(RunArgs {
                    command: RunCommand::Artifacts(args),
                }),
            )
        }
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
        connection: apply_context(connection, request, cwd)?,
        command,
    })
}

/// A relative text-file argument is resolved against the execution cwd.
fn resolve_file(path: Option<String>, cwd: &Path) -> Option<String> {
    path.map(|value| {
        let path = Path::new(&value);
        if path.is_absolute() {
            value
        } else {
            cwd.join(path).to_string_lossy().into_owned()
        }
    })
}

/// Fill in what the caller cannot supply: the cwd, the request deadline, and a
/// token resolved from the vault.
fn apply_context(
    mut connection: GithubConnectionArgs,
    request: &TypedInvocationRequest,
    cwd: PathBuf,
) -> Result<GithubConnectionArgs, CommandError> {
    let inline = connection.token.take();
    connection.token = connection_token(request, inline)?;
    connection.timeout_secs = connection.timeout_secs.min(remaining_seconds(request));
    connection.cwd = Some(cwd);
    Ok(connection)
}

/// The vault credential and an inline token are mutually exclusive; a resolved
/// credential always wins over nothing, never over an explicit argument.
fn connection_token(
    request: &TypedInvocationRequest,
    inline: Option<String>,
) -> Result<Option<String>, CommandError> {
    let resolved = resolved_token(request)?;
    if inline.is_some() && resolved.is_some() {
        return Err(command_error(
            request,
            "INVALID_ARGUMENT",
            "GitHub token credential conflicts with an inline token",
            "select either a vault credential or an inline token",
            false,
        ));
    }
    Ok(resolved.or(inline))
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

/// Binds the vault token slot: the public credential id and the privately
/// resolved secret must both be present and must agree.
fn resolved_token(request: &TypedInvocationRequest) -> Result<Option<String>, CommandError> {
    let credentials = request
        .arguments
        .get("credentials")
        .and_then(Value::as_object);
    if let Some(slot) = credentials.and_then(|items| items.keys().find(|key| *key != "token")) {
        return Err(command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Unsupported GitHub credential slot '{slot}'"),
            "only the token credential slot is supported",
            false,
        ));
    }
    let public_id = credentials
        .and_then(|items| items.get("token"))
        .and_then(Value::as_str);
    let token = token_from_resolved_secrets(&request.resolved_secrets).map_err(|response| {
        command_error(
            request,
            response
                .error_code
                .unwrap_or_else(|| "INVALID_ARGUMENT".to_owned()),
            response
                .error_message
                .unwrap_or_else(|| "credential resolution failed".to_owned()),
            "credential slot or kind mismatch",
            false,
        )
    })?;
    match (public_id, &token) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(command_error(
                request,
                "SECRET_REQUIRED",
                "GitHub token credential was not resolved",
                "public credential selection and private resolution must both be present",
                false,
            ));
        }
    }
    if let Some((public_id, secret)) = public_id.zip(request.resolved_secrets.get("token"))
        && secret.id != public_id
    {
        return Err(command_error(
            request,
            "SECRET_KIND_MISMATCH",
            "Resolved GitHub token does not match the selected credential",
            "credential identity or kind mismatch",
            false,
        ));
    }
    Ok(token)
}

/// Shared by the typed and direct CLI paths: validates the slot and shape of the
/// credential the host resolved.
pub(super) fn token_from_resolved_secrets(
    secrets: &std::collections::BTreeMap<String, ah_plugin_api::ResolvedSecret>,
) -> Result<Option<String>, ah_plugin_api::InvocationResponse> {
    if let Some(slot) = secrets.keys().find(|slot| slot.as_str() != "token") {
        return Err(ah_plugin_api::InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("Unsupported resolved GitHub credential slot '{slot}'"),
        ));
    }
    let Some(resolved) = secrets.get("token") else {
        return Ok(None);
    };
    if resolved.kind != "github-token" {
        return Err(ah_plugin_api::InvocationResponse::error(
            "SECRET_KIND_MISMATCH",
            "Resolved GitHub token does not match the selected credential",
        ));
    }
    let token = resolved.values.get("token").ok_or_else(|| {
        ah_plugin_api::InvocationResponse::error(
            "SECRET_REQUIRED",
            "Resolved GitHub credential has no token",
        )
    })?;
    if token.trim().is_empty() {
        return Err(ah_plugin_api::InvocationResponse::error(
            "INVALID_ARGUMENT",
            "Resolved GitHub token must not be empty",
        ));
    }
    Ok(Some(token.clone()))
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

fn repo_descriptor() -> CommandDescriptor {
    descriptor(
        "github.repo",
        "Inspect GitHub repository",
        "Detect the GitHub repository and return remote plus API metadata.",
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<RepoOutput>("github.repo"),
        read_effects(
            "May run Git repository detection and sends a read request to the configured API URL; a supplied token is sent to that host.",
        ),
    )
}

fn issues_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issues",
        "List GitHub issues",
        "List or search repository issues with filters and the shared result limit.",
        input_schema_for::<Wire<IssuesArgs>>(),
        output_schema_for::<IssuesOutput>("github.issues"),
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
        input_schema_for::<Wire<IssueNumberArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.view"),
        read_effects("Reads one issue and its metadata from the configured GitHub API."),
    )
}

fn issue_create_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.create",
        "Create GitHub issue",
        "Create a repository issue with optional body, labels, and assignees. Use body or body_file, not both.",
        input_schema_for::<Wire<CreateIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.create"),
        write_effects(
            "Creates a persistent issue and may notify repository participants; body files are read from the execution cwd.",
        ),
    )
}

fn issue_update_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.update",
        "Update GitHub issue",
        "Update one or more issue fields. At least one update field is required; use body or body_file, not both.",
        input_schema_for::<Wire<UpdateIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.update"),
        write_effects(
            "Mutates a persistent issue and may change workflow state or notify participants.",
        ),
    )
}

fn issue_close_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.close",
        "Close GitHub issue",
        "Close an issue, optionally adding a comment first. Use comment or comment_file, not both.",
        input_schema_for::<Wire<CloseIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.close"),
        write_effects(
            "May create a comment, closes a persistent issue, and may notify participants.",
        ),
    )
}

fn issue_comment_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.comment",
        "Comment on GitHub issue",
        "Create a comment on one repository issue. Exactly one of body or body_file is required.",
        input_schema_for::<Wire<CommentIssueArgs>>(),
        output_schema_for::<IssueCommentOutput>("github.issue.comment"),
        write_effects("Creates a persistent issue comment and may notify repository participants."),
    )
}

fn issue_comments_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.comments",
        "List GitHub issue comments",
        "List comments for one issue with the shared result limit.",
        input_schema_for::<Wire<IssueNumberArgs>>(),
        output_schema_for::<IssueCommentsOutput>("github.issue.comments"),
        read_effects("Reads issue comments and author metadata from the configured GitHub API."),
    )
}

fn release_get_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.get",
        "Get GitHub release",
        "Return GitHub release metadata by tag.",
        input_schema_for::<Wire<TagArgs>>(),
        output_schema_for::<ReleaseOutput>("github.release.get"),
        read_effects("Reads release metadata and asset URLs from the configured GitHub API."),
    )
}

fn release_assets_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.assets",
        "List GitHub release assets",
        "List assets attached to a release tag.",
        input_schema_for::<Wire<TagArgs>>(),
        output_schema_for::<ReleaseAssetsOutput>("github.release.assets"),
        read_effects(
            "Reads release asset metadata and download URLs from the configured GitHub API.",
        ),
    )
}

fn release_create_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.create",
        "Create GitHub release",
        "Create a GitHub release for a tag with optional notes and flags. Use notes or notes_file, not both.",
        input_schema_for::<Wire<CreateReleaseArgs>>(),
        output_schema_for::<ReleaseOutput>("github.release.create"),
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
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<WorkflowsOutput>("github.workflows"),
        read_effects(
            "Reads workflow names, paths, states, and URLs from the configured GitHub API.",
        ),
    )
}

fn workflow_run_descriptor() -> CommandDescriptor {
    descriptor(
        "github.workflow.run",
        "Dispatch GitHub workflow",
        "Dispatch a GitHub Actions workflow on a reference. Dispatching needs a token carrying the actions write scope, which the git credential helper usually does not: on HTTP 401 stop and ask the user for a github-token secret rather than retrying.",
        input_schema_for::<Wire<WorkflowRunArgs>>(),
        output_schema_for::<WorkflowDispatchOutput>("github.workflow.run"),
        write_effects(
            "Starts an external workflow that may execute arbitrary repository automation and consume billed resources.",
        ),
    )
}

fn runs_descriptor() -> CommandDescriptor {
    descriptor(
        "github.runs",
        "List GitHub workflow runs",
        "List workflow runs with optional workflow and branch filters.",
        input_schema_for::<Wire<RunsArgs>>(),
        output_schema_for::<RunsOutput>("github.runs"),
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
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<RunOutput>("github.run.get"),
        read_effects(
            "Reads one workflow run and its commit/status metadata from the configured GitHub API.",
        ),
    )
}

fn run_wait_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.wait",
        "Wait for GitHub workflow run",
        "Poll a workflow run until completion, timeout, or cancellation.",
        input_schema_for::<Wire<WaitRunArgs>>(),
        output_schema_for::<WaitRunOutput>("github.run.wait"),
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
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<JobsOutput>("github.run.jobs"),
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
    descriptor(
        id,
        if warnings {
            "Extract GitHub run warnings"
        } else {
            "Search GitHub run logs"
        },
        if warnings {
            "Extract warning-like lines from one workflow run log archive. The archive exists only once the run has finished and until GitHub expires it, so an unfinished or expired run answers 404: check github.run.get, or wait with github.run.wait, instead of retrying."
        } else {
            "Read or filter lines from one workflow run log archive. The archive exists only once the run has finished and until GitHub expires it, so an unfinished or expired run answers 404: check github.run.get, or wait with github.run.wait, instead of retrying."
        },
        if warnings {
            input_schema_for::<Wire<LogReadArgs>>()
        } else {
            input_schema_for::<Wire<LogArgs>>()
        },
        output_schema_for::<LogsOutput>(id),
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
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<ArtifactsOutput>("github.run.artifacts"),
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
        .with_secret_slot(token_slot())
}

/// Every command in this domain talks to the API, so all of them accept the
/// vault token slot.
fn token_slot() -> SecretSlot {
    SecretSlot::optional("token", ["github-token"], "GitHub API token.")
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ah_plugin_api::{ExecutionContextWire, ResolvedSecret};
    use serde_json::json;

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
    fn every_command_declares_only_the_token_secret_slot() {
        for command in &command_catalog().commands {
            assert_eq!(command.secret_slots.len(), 1, "{}", command.id);
            let slot = &command.secret_slots[0];
            assert_eq!(slot.name, "token");
            assert_eq!(slot.accepted_kinds, ["github-token"]);
            assert!(!slot.required);
        }
    }

    /// The connection block is no longer built by a function of its own; it
    /// comes out of the decoded wire type, so exercise it through that.
    fn connection_of(
        request: &TypedInvocationRequest,
    ) -> Result<GithubConnectionArgs, CommandError> {
        typed_cli(request).map(|cli| cli.connection)
    }

    #[test]
    fn typed_connection_binds_a_matching_resolved_token() {
        let request =
            token_request(json!({"repo": "owner/repo"})).with_resolved_secrets(BTreeMap::from([(
                "token".to_owned(),
                resolved_secret("github-token", "vault-token-sentinel"),
            )]));

        let connection = connection_of(&request).expect("credential should bind");

        assert_eq!(connection.token.as_deref(), Some("vault-token-sentinel"));
    }

    #[test]
    fn typed_connection_rejects_an_inline_token_beside_a_credential() {
        let mut arguments = json!({"repo": "owner/repo"});
        arguments["token"] = json!("inline-token-sentinel");
        let request = TypedInvocationRequest::new(
            "github.repo",
            {
                arguments["credentials"] = json!({"token": "api"});
                arguments
            },
            ExecutionContextWire::new("github-token-conflict", ".", None, 2_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "token".to_owned(),
            resolved_secret("github-token", "vault-token-sentinel"),
        )]));

        let error = connection_of(&request).expect_err("token sources must conflict");
        let serialized = serde_json::to_string(&error).expect("error serializes");

        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("inline-token-sentinel"));
        assert!(!serialized.contains("vault-token-sentinel"));
    }

    #[test]
    fn typed_connection_rejects_unresolved_and_mismatched_credentials() {
        let unresolved = token_request(json!({"repo": "owner/repo"}));
        assert_eq!(
            connection_of(&unresolved)
                .expect_err("public id requires private resolution")
                .code,
            "SECRET_REQUIRED"
        );

        let wrong_kind =
            token_request(json!({"repo": "owner/repo"})).with_resolved_secrets(BTreeMap::from([(
                "token".to_owned(),
                resolved_secret("http-basic", "wrong-kind-sentinel"),
            )]));
        let error = connection_of(&wrong_kind).expect_err("kind must match");
        let serialized = serde_json::to_string(&error).expect("error serializes");

        assert_eq!(error.code, "SECRET_KIND_MISMATCH");
        assert!(!serialized.contains("wrong-kind-sentinel"));
    }

    fn token_request(mut arguments: Value) -> TypedInvocationRequest {
        arguments["credentials"] = json!({"token": "api"});
        TypedInvocationRequest::new(
            "github.repo",
            arguments,
            ExecutionContextWire::new("github-token", ".", None, 2_000),
        )
    }

    fn resolved_secret(kind: &str, token: &str) -> ResolvedSecret {
        ResolvedSecret {
            id: "api".to_owned(),
            kind: kind.to_owned(),
            values: BTreeMap::from([("token".to_owned(), token.to_owned())]),
        }
    }

    #[test]
    fn typed_commands_use_git_credentials_by_default() {
        let request = TypedInvocationRequest::new(
            "github.repo",
            json!({"repo": "owner/repo"}),
            ExecutionContextWire::new("github-default-auth", ".", None, 2_000),
        );

        assert!(connection_of(&request).unwrap().use_git_credential);
        assert_eq!(
            command_catalog().commands[0].input_schema["properties"]["use_git_credential"]["default"],
            true
        );
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
        let request_id = "github-cancel-test";
        let _scope = cancellation::RequestScope::enter(request_id);
        assert!(cancellation::cancel(request_id));
        assert!(cancellation::wait_or_cancel(Duration::from_secs(1)));
    }

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "github-pre-cancelled";
        assert!(cancellation::cancel(request_id));
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
        assert!(!cancellation::is_cancelled());
    }
}
