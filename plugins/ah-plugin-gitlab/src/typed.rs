use std::path::{Path, PathBuf};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    GlobalOptionsWire, Reversibility, RiskLevel, SecretSlot, TypedInvocationRequest,
    TypedInvocationResponse, cancellation,
    schema::{input_schema_for, output_schema_for},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::*;

mod catalog;
pub(super) use catalog::command_catalog;
use catalog::token_slot;

// Every GitLab command takes the same connection block plus its own arguments,
// so one generic wire type describes all of them. A doc comment here would be
// published as the schema `description`; `deny_unknown_fields` cannot be
// combined with `flatten`, so the closed-object rule is stated for the schema
// and the runtime rejects unknown properties against it before dispatch.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
struct Wire<T> {
    #[serde(flatten)]
    connection: GitlabConnectionArgs,
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
            "GitLab command arguments are invalid",
            error.to_string(),
            false,
        )
    })
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
        "GitLab command execution was cancelled",
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

fn typed_cli(request: &TypedInvocationRequest) -> Result<GitlabCli, CommandError> {
    let cwd = PathBuf::from(&request.context.cwd);

    macro_rules! decoded {
        ($args:ty) => {{
            let wire: Wire<$args> = decode(request)?;
            (wire.connection, wire.command)
        }};
    }

    let (connection, command) = match request.command.as_str() {
        "gitlab.project" => (decoded!(NoArgs).0, GitlabCommand::Project),
        "gitlab.releases" => (decoded!(NoArgs).0, GitlabCommand::Releases),
        "gitlab.release.get" => {
            let (connection, args) = decoded!(TagArgs);
            (
                connection,
                GitlabCommand::Release(ReleaseArgs {
                    command: ReleaseCommand::Get(args),
                }),
            )
        }
        "gitlab.release.create" => {
            let (connection, mut args) = decoded!(CreateReleaseArgs);
            args.description_file = resolve_file(args.description_file, &cwd);
            (
                connection,
                GitlabCommand::Release(ReleaseArgs {
                    command: ReleaseCommand::Create(args),
                }),
            )
        }
        "gitlab.issues" => {
            let (connection, args) = decoded!(IssuesArgs);
            (connection, GitlabCommand::Issues(args))
        }
        "gitlab.issue.view" => {
            let (connection, args) = decoded!(IssueViewArgs);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::View(args),
                }),
            )
        }
        "gitlab.issue.create" => {
            let (connection, mut args) = decoded!(CreateIssueArgs);
            args.description_file = resolve_file(args.description_file, &cwd);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::Create(args),
                }),
            )
        }
        "gitlab.issue.update" => {
            let (connection, mut args) = decoded!(UpdateIssueArgs);
            args.description_file = resolve_file(args.description_file, &cwd);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::Update(args),
                }),
            )
        }
        "gitlab.issue.close" => {
            let (connection, mut args) = decoded!(CloseIssueArgs);
            args.comment_file = resolve_file(args.comment_file, &cwd);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::Close(args),
                }),
            )
        }
        "gitlab.issue.comment" => {
            let (connection, mut args) = decoded!(CommentIssueArgs);
            args.body_file = resolve_file(args.body_file, &cwd);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::Comment(args),
                }),
            )
        }
        "gitlab.issue.comments" => {
            let (connection, args) = decoded!(IssueIidArgs);
            (
                connection,
                GitlabCommand::Issue(IssueArgs {
                    command: IssueCommand::Comments(args),
                }),
            )
        }
        "gitlab.pipelines" => {
            let (connection, args) = decoded!(PipelinesArgs);
            (connection, GitlabCommand::Pipelines(args))
        }
        "gitlab.pipeline.get" => {
            let (connection, args) = decoded!(PipelineIdArgs);
            (
                connection,
                GitlabCommand::Pipeline(PipelineArgs {
                    command: PipelineCommand::Get(args),
                }),
            )
        }
        "gitlab.pipeline.wait" => {
            let (connection, mut args) = decoded!(WaitPipelineArgs);
            args.timeout_secs = args.timeout_secs.min(remaining_seconds(request));
            (
                connection,
                GitlabCommand::Pipeline(PipelineArgs {
                    command: PipelineCommand::Wait(args),
                }),
            )
        }
        "gitlab.pipeline.jobs" => {
            let (connection, args) = decoded!(PipelineIdArgs);
            (
                connection,
                GitlabCommand::Pipeline(PipelineArgs {
                    command: PipelineCommand::Jobs(args),
                }),
            )
        }
        "gitlab.job.trace" => {
            let (connection, args) = decoded!(JobTraceArgs);
            (
                connection,
                GitlabCommand::Job(JobArgs {
                    command: JobCommand::Trace(args),
                }),
            )
        }
        "gitlab.job.warnings" => {
            let (connection, args) = decoded!(JobTraceReadArgs);
            (
                connection,
                GitlabCommand::Job(JobArgs {
                    command: JobCommand::Warnings(args),
                }),
            )
        }
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
    mut connection: GitlabConnectionArgs,
    request: &TypedInvocationRequest,
    cwd: PathBuf,
) -> Result<GitlabConnectionArgs, CommandError> {
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
            "GitLab token credential conflicts with an inline token",
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
            format!("Unsupported GitLab credential slot '{slot}'"),
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
                "GitLab token credential was not resolved",
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
            "Resolved GitLab token does not match the selected credential",
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
            format!("Unsupported resolved GitLab credential slot '{slot}'"),
        ));
    }
    let Some(resolved) = secrets.get("token") else {
        return Ok(None);
    };
    if resolved.kind != "gitlab-token" {
        return Err(ah_plugin_api::InvocationResponse::error(
            "SECRET_KIND_MISMATCH",
            "Resolved GitLab token does not match the selected credential",
        ));
    }
    let token = resolved.values.get("token").ok_or_else(|| {
        ah_plugin_api::InvocationResponse::error(
            "SECRET_REQUIRED",
            "Resolved GitLab credential has no token",
        )
    })?;
    if token.trim().is_empty() {
        return Err(ah_plugin_api::InvocationResponse::error(
            "INVALID_ARGUMENT",
            "Resolved GitLab token must not be empty",
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ah_plugin_api::{ExecutionContextWire, ResolvedSecret};

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
    fn every_command_declares_only_the_token_secret_slot() {
        for command in &command_catalog().commands {
            assert_eq!(command.secret_slots.len(), 1, "{}", command.id);
            let slot = &command.secret_slots[0];
            assert_eq!(slot.name, "token");
            assert_eq!(slot.accepted_kinds, ["gitlab-token"]);
            assert!(!slot.required);
        }
    }

    /// The connection block is no longer built by a function of its own; it
    /// comes out of the decoded wire type, so exercise it through that.
    fn connection_of(
        request: &TypedInvocationRequest,
    ) -> Result<GitlabConnectionArgs, CommandError> {
        typed_cli(request).map(|cli| cli.connection)
    }

    #[test]
    fn typed_connection_binds_a_matching_resolved_token() {
        let request = token_request(json!({"project": "group/project"})).with_resolved_secrets(
            BTreeMap::from([(
                "token".to_owned(),
                resolved_secret("gitlab-token", "vault-token-sentinel"),
            )]),
        );

        let connection = connection_of(&request).expect("credential should bind");

        assert_eq!(connection.token.as_deref(), Some("vault-token-sentinel"));
    }

    #[test]
    fn typed_connection_rejects_an_inline_token_beside_a_credential() {
        let mut arguments = json!({"project": "group/project"});
        arguments["token"] = json!("inline-token-sentinel");
        let request = TypedInvocationRequest::new(
            "gitlab.project",
            {
                arguments["credentials"] = json!({"token": "api"});
                arguments
            },
            ExecutionContextWire::new("gitlab-token-conflict", ".", None, 2_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "token".to_owned(),
            resolved_secret("gitlab-token", "vault-token-sentinel"),
        )]));

        let error = connection_of(&request).expect_err("token sources must conflict");
        let serialized = serde_json::to_string(&error).expect("error serializes");

        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("inline-token-sentinel"));
        assert!(!serialized.contains("vault-token-sentinel"));
    }

    #[test]
    fn typed_connection_rejects_unresolved_and_mismatched_credentials() {
        let unresolved = token_request(json!({"project": "group/project"}));
        assert_eq!(
            connection_of(&unresolved)
                .expect_err("public id requires private resolution")
                .code,
            "SECRET_REQUIRED"
        );

        let wrong_kind = token_request(json!({"project": "group/project"})).with_resolved_secrets(
            BTreeMap::from([(
                "token".to_owned(),
                resolved_secret("http-basic", "wrong-kind-sentinel"),
            )]),
        );
        let error = connection_of(&wrong_kind).expect_err("kind must match");
        let serialized = serde_json::to_string(&error).expect("error serializes");

        assert_eq!(error.code, "SECRET_KIND_MISMATCH");
        assert!(!serialized.contains("wrong-kind-sentinel"));
    }

    fn token_request(mut arguments: Value) -> TypedInvocationRequest {
        arguments["credentials"] = json!({"token": "api"});
        TypedInvocationRequest::new(
            "gitlab.project",
            arguments,
            ExecutionContextWire::new("gitlab-token", ".", None, 2_000),
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
            "gitlab.project",
            json!({"project": "group/project"}),
            ExecutionContextWire::new("gitlab-default-auth", ".", None, 2_000),
        );

        assert!(connection_of(&request).unwrap().use_git_credential);
        assert_eq!(
            command_catalog().commands[0].input_schema["properties"]["use_git_credential"]["default"],
            true
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
        let request_id = "gitlab-cancel-test";
        let _scope = cancellation::RequestScope::enter(request_id);
        assert!(cancellation::cancel(request_id));
        assert!(cancellation::wait_or_cancel(Duration::from_secs(1)));
    }

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "gitlab-pre-cancelled";
        assert!(cancellation::cancel(request_id));
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
        assert!(!cancellation::is_cancelled());
    }
}
