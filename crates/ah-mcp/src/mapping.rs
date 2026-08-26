//! Turning catalog commands, typed responses and errors into MCP types.

use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc};

use crate::job_tools::job_tools;
use crate::server::{
    CatalogSnapshot, DIAGNOSTIC_META_KEY, EXECUTION_META_KEY, JOB_TOOL_PREFIX, McpAdapterError,
    McpCommandStatus, McpServerConfig, RISK_META_KEY, TOOL_PREFIX,
};

use ah_plugin_api::{
    CommandDescriptor, CommandError, ExecutionContextWire, TypedInvocationResponse,
};
use ah_runtime::{InvocationOutcome, PluginManager, RegisteredCommand, RunCheckOutcome};
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorCode, JsonObject, Meta, NumberOrString, TaskSupport, Tool,
    ToolAnnotations, ToolExecution,
};
use serde_json::{Map, Value};

pub(crate) fn command_to_tool(command: &RegisteredCommand) -> Result<Tool, McpAdapterError> {
    let descriptor = &command.descriptor;
    let input_schema = schema_object_with_context(descriptor)?;
    let output_schema = schema_object(&descriptor.id, "output", &descriptor.output_schema)?;
    let risk =
        serde_json::to_value(descriptor.effects.risk).expect("risk enum should always serialize");
    let reversibility = serde_json::to_value(descriptor.effects.reversibility)
        .expect("reversibility enum should always serialize");
    let effects = serde_json::to_value(&descriptor.effects.effects)
        .expect("effect enums should always serialize");
    let mut risk_meta = Map::new();
    risk_meta.insert("level".to_owned(), risk.clone());
    risk_meta.insert(
        "impact".to_owned(),
        Value::String(descriptor.effects.impact.clone()),
    );
    risk_meta.insert("effects".to_owned(), effects);
    risk_meta.insert("reversibility".to_owned(), reversibility);
    let mut meta = Map::new();
    meta.insert(RISK_META_KEY.to_owned(), Value::Object(risk_meta));

    let risk_label = risk.as_str().unwrap_or("unknown");
    let examples = descriptor
        .examples
        .iter()
        .map(|example| format!("- {}: {}", example.description, example.arguments))
        .collect::<Vec<_>>()
        .join("\n");
    let example_section = (!examples.is_empty()).then(|| format!("\n\nExamples:\n{examples}"));
    let credential_section = (!descriptor.secret_slots.is_empty()).then(|| {
        let slots = descriptor
            .secret_slots
            .iter()
            .map(|slot| {
                let kinds = slot.accepted_kinds.join(", ");
                let filters = slot
                    .accepted_kinds
                    .iter()
                    .map(|kind| format!("kind={kind}"))
                    .collect::<Vec<_>>()
                    .join(" or ");
                format!(
                    "{} accepts {kinds}. If the ID is unknown, call secrets.list with {filters}.",
                    slot.name
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!("\n\nCredential slots: {slots}")
    });
    let description = format!(
        "{}{}\n\nFor project-relative paths, include context.cwd. HTTP job targets always require an absolute context.cwd.{}\n\nImpact: {}\nRisk: {risk_label}.",
        descriptor.description,
        credential_section.unwrap_or_default(),
        example_section.unwrap_or_default(),
        descriptor.effects.impact
    );
    let mut tool = Tool::new(
        format!("{TOOL_PREFIX}{}", descriptor.id),
        description,
        input_schema,
    );
    tool.title = Some(descriptor.title.clone());
    tool.output_schema = Some(Arc::new(output_schema));
    tool.annotations = Some(ToolAnnotations::from_raw(
        Some(descriptor.title.clone()),
        Some(descriptor.effects.read_only),
        Some(descriptor.effects.destructive),
        Some(descriptor.effects.idempotent),
        Some(descriptor.effects.open_world),
    ));
    tool.execution = Some(ToolExecution::new().with_task_support(TaskSupport::Forbidden));
    tool.meta = Some(Meta(meta));
    Ok(tool)
}

pub(crate) fn schema_object_with_context(
    descriptor: &CommandDescriptor,
) -> Result<JsonObject, McpAdapterError> {
    let schema = ah_runtime::typed::mcp_input_schema(descriptor).map_err(|error| {
        McpAdapterError::InvalidSchema {
            command: descriptor.id.clone(),
            reason: error.to_string(),
        }
    })?;
    schema_object(&descriptor.id, "input", &schema)
}

pub(crate) fn schema_object(
    command: &str,
    kind: &str,
    schema: &Value,
) -> Result<JsonObject, McpAdapterError> {
    schema
        .as_object()
        .cloned()
        .ok_or_else(|| McpAdapterError::InvalidSchema {
            command: command.to_owned(),
            reason: format!("{kind} schema root must be an object"),
        })
}

pub(crate) fn extract_context(
    arguments: &mut JsonObject,
    request_id: &str,
    defaults: &McpServerConfig,
    descriptor: &CommandDescriptor,
    require_explicit_cwd: bool,
) -> Result<ExecutionContextWire, CommandError> {
    let context = arguments.remove("context");
    let Some(context) = context else {
        if require_explicit_cwd {
            return Err(context_error(
                descriptor,
                "context.cwd is required for HTTP execution",
            ));
        }
        return Ok(ExecutionContextWire::new(
            request_id,
            defaults.cwd.clone(),
            defaults.limit,
            defaults.default_timeout_ms,
        ));
    };
    let Some(context) = context.as_object() else {
        return Err(context_error(descriptor, "context must be a JSON object"));
    };
    for key in context.keys() {
        if !matches!(key.as_str(), "cwd" | "limit" | "timeout_ms") {
            return Err(context_error(
                descriptor,
                format!("unknown context property '{key}'"),
            ));
        }
    }

    let cwd = match context.get("cwd") {
        Some(Value::String(cwd)) if !cwd.trim().is_empty() => cwd.clone(),
        Some(_) => {
            return Err(context_error(
                descriptor,
                "context.cwd must be a non-empty string",
            ));
        }
        None if require_explicit_cwd => {
            return Err(context_error(
                descriptor,
                "context.cwd is required for HTTP execution",
            ));
        }
        None => defaults.cwd.clone(),
    };
    if require_explicit_cwd && !Path::new(&cwd).is_absolute() {
        return Err(context_error(
            descriptor,
            "context.cwd must be an absolute path for HTTP execution",
        ));
    }
    let limit = match context.get("limit") {
        Some(value) => Some(positive_usize(value, "context.limit", descriptor)?),
        None => defaults.limit,
    };
    let timeout_ms = match context.get("timeout_ms") {
        Some(value) => positive_u64(value, "context.timeout_ms", descriptor)?,
        None => defaults.default_timeout_ms,
    };
    Ok(ExecutionContextWire::new(
        request_id, cwd, limit, timeout_ms,
    ))
}

pub(crate) fn requires_explicit_cwd(
    descriptor: &CommandDescriptor,
    arguments: &JsonObject,
    http_transport: bool,
) -> bool {
    if !http_transport {
        return false;
    }

    match descriptor.id.as_str() {
        "ai.info" | "plugins.list" | "plugins.enable" | "plugins.disable" | "plugins.reset" => {
            false
        }
        command if command.starts_with("ollama.") => false,
        command if command.starts_with("postgres.") => {
            has_relative_path(arguments, "tool_path")
                || (command == "postgres.tool.use" && has_relative_path(arguments, "path"))
        }
        "http.assert" | "http.run" => true,
        command if command.starts_with("http.") => {
            has_relative_path(arguments, "json_file") || has_relative_path(arguments, "body_file")
        }
        // A hosted API call reads nothing from disk once the caller names the
        // project itself, and its credential lookup is bound to the API host
        // rather than to a git remote.
        command if command.starts_with("github.") => {
            !has_text(arguments, "repo") || has_relative_input_file(arguments)
        }
        command if command.starts_with("gitlab.") => {
            !has_text(arguments, "project") || has_relative_input_file(arguments)
        }
        _ => true,
    }
}

/// The file-backed inputs a hosted API call can carry, all of them resolved
/// against the working directory.
pub(crate) fn has_relative_input_file(arguments: &JsonObject) -> bool {
    [
        "body_file",
        "comment_file",
        "description_file",
        "notes_file",
    ]
    .into_iter()
    .any(|field| has_relative_path(arguments, field))
}

pub(crate) fn has_text(arguments: &JsonObject, field: &str) -> bool {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn has_relative_path(arguments: &JsonObject, field: &str) -> bool {
    arguments
        .get(field)
        .and_then(Value::as_str)
        .is_some_and(|path| !Path::new(path).is_absolute())
}

pub(crate) fn positive_usize(
    value: &Value,
    field: &str,
    descriptor: &CommandDescriptor,
) -> Result<usize, CommandError> {
    let value = positive_u64(value, field, descriptor)?;
    usize::try_from(value).map_err(|_| context_error(descriptor, format!("{field} is too large")))
}

pub(crate) fn positive_u64(
    value: &Value,
    field: &str,
    descriptor: &CommandDescriptor,
) -> Result<u64, CommandError> {
    value
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| context_error(descriptor, format!("{field} must be a positive integer")))
}

pub(crate) fn context_error(
    descriptor: &CommandDescriptor,
    cause: impl Into<String>,
) -> CommandError {
    CommandError::new(
        command_domain(&descriptor.id),
        Some(descriptor.id.clone()),
        "INVALID_CONTEXT",
        "Invalid MCP execution context",
        cause,
        2,
        false,
    )
}

pub(crate) fn typed_response_result(
    response: TypedInvocationResponse,
    request_id: &str,
) -> CallToolResult {
    if !response.success {
        return command_error_result(response.error.unwrap_or_else(|| {
            CommandError::new(
                None,
                None,
                "INVALID_TYPED_RESPONSE",
                "Typed command returned an invalid error response",
                "success=false without a diagnostic",
                1,
                false,
            )
        }));
    }
    let Some(data) = response.data else {
        return command_error_result(CommandError::new(
            None,
            None,
            "INVALID_TYPED_RESPONSE",
            "Typed command returned an invalid success response",
            "success=true without structured data",
            1,
            false,
        ));
    };
    let compact = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_owned());
    let mut result = CallToolResult::structured(data);
    result.content = vec![ContentBlock::text(compact)];
    let mut execution = Map::new();
    execution.insert(
        "request_id".to_owned(),
        Value::String(request_id.to_owned()),
    );
    if let Some(text) = response.text {
        execution.insert("text".to_owned(), Value::String(text));
    }
    if !response.notices.is_empty() {
        execution.insert(
            "notices".to_owned(),
            serde_json::to_value(response.notices)
                .expect("command notices should always serialize"),
        );
    }
    let mut meta = Map::new();
    meta.insert(EXECUTION_META_KEY.to_owned(), Value::Object(execution));
    result.meta = Some(Meta(meta));
    result
}

pub(crate) fn command_error_result(error: CommandError) -> CallToolResult {
    let text = format!("{}: {}", error.code, error.message);
    let mut result = CallToolResult::error(vec![ContentBlock::text(text)]);
    let mut meta = Map::new();
    meta.insert(
        DIAGNOSTIC_META_KEY.to_owned(),
        serde_json::to_value(error).expect("command error should always serialize"),
    );
    result.meta = Some(Meta(meta));
    result
}

pub(crate) fn command_event_outcome(
    result: &Result<CallToolResult, rmcp::ErrorData>,
    canonical_command: Option<&str>,
) -> (McpCommandStatus, Option<CommandError>) {
    match result {
        Ok(result) if result.is_error == Some(true) => {
            let diagnostic = result
                .meta
                .as_ref()
                .and_then(|meta| meta.0.get(DIAGNOSTIC_META_KEY))
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_else(|| {
                    adapter_command_error(
                        canonical_command,
                        "MCP_ERROR_DIAGNOSTIC_MISSING",
                        "MCP tool returned an error without a diagnostic",
                        "the CallToolResult diagnostic metadata was missing or invalid",
                    )
                });
            (McpCommandStatus::Error, Some(diagnostic))
        }
        Ok(_) => (McpCommandStatus::Success, None),
        Err(error) => (
            McpCommandStatus::Error,
            Some(adapter_command_error(
                canonical_command,
                "MCP_PROTOCOL_ERROR",
                "MCP tool call failed",
                error.to_string(),
            )),
        ),
    }
}

pub(crate) fn command_event_run_check_outcome(
    result: &Result<CallToolResult, rmcp::ErrorData>,
    canonical_command: Option<&str>,
) -> Option<InvocationOutcome> {
    let result = result.as_ref().ok()?;
    if result.is_error == Some(true) {
        return None;
    }
    run_check_outcome(canonical_command?, result.structured_content.as_ref())
}

pub(crate) fn run_check_outcome(
    canonical_command: &str,
    data: Option<&Value>,
) -> Option<InvocationOutcome> {
    if canonical_command != "run.check" {
        return None;
    }
    let data = data?;
    let success = data.get("success")?.as_bool()?;
    let timed_out = data.get("timed_out")?.as_bool()?;
    let exit_code = match data.get("exit_code")? {
        Value::Null => None,
        value => Some(i32::try_from(value.as_i64()?).ok()?),
    };
    Some(InvocationOutcome::RunCheck(RunCheckOutcome {
        success,
        timed_out,
        exit_code,
    }))
}

pub(crate) fn adapter_command_error(
    canonical_command: Option<&str>,
    code: &'static str,
    message: &'static str,
    cause: impl Into<String>,
) -> CommandError {
    CommandError::new(
        canonical_command.and_then(command_domain),
        canonical_command.map(str::to_owned),
        code,
        message,
        cause,
        1,
        false,
    )
}

pub(crate) fn command_domain(command: &str) -> Option<String> {
    command.split_once('.').map(|(domain, _)| domain.to_owned())
}

pub(crate) fn execution_request_id(request_id: &NumberOrString) -> String {
    match request_id {
        NumberOrString::Number(value) => format!("mcp:n:{value}"),
        NumberOrString::String(value) => format!("mcp:s:{value}"),
    }
}

pub(crate) fn unknown_tool_error(name: &str) -> rmcp::ErrorData {
    rmcp::ErrorData::new(
        ErrorCode::METHOD_NOT_FOUND,
        format!("unknown MCP tool '{name}'"),
        None,
    )
}

pub(crate) fn internal_catalog_error(error: impl std::fmt::Display) -> rmcp::ErrorData {
    rmcp::ErrorData::internal_error(
        Cow::Owned(format!("AIHelper command catalog failed: {error}")),
        None,
    )
}

pub(crate) fn build_catalog_snapshot(
    manager: &PluginManager,
) -> Result<CatalogSnapshot, McpAdapterError> {
    loop {
        let runtime_revision = manager.catalog_revision();
        let commands = manager.list_enabled_commands()?;
        let mut tools = Vec::with_capacity(commands.len() + 4);
        let mut tools_by_name = HashMap::with_capacity(commands.len() + 4);
        let mut commands_by_name = HashMap::with_capacity(commands.len());
        for command in commands {
            let name = format!("{TOOL_PREFIX}{}", command.descriptor.id);
            if let Some(error) = reserved_job_namespace_error(&command.descriptor.id) {
                return Err(error);
            }
            let tool = command_to_tool(&command)?;
            tools_by_name.insert(name.clone(), tool.clone());
            commands_by_name.insert(name, command);
            tools.push(tool);
        }
        for tool in job_tools() {
            tools_by_name.insert(tool.name.to_string(), tool.clone());
            tools.push(tool);
        }
        if manager.catalog_revision() == runtime_revision {
            return Ok(CatalogSnapshot {
                runtime_revision,
                tools,
                tools_by_name,
                commands_by_name,
            });
        }
    }
}

pub(crate) fn reserved_job_namespace_error(command: &str) -> Option<McpAdapterError> {
    format!("{TOOL_PREFIX}{command}")
        .starts_with(JOB_TOOL_PREFIX)
        .then(|| McpAdapterError::InvalidSchema {
            command: command.to_owned(),
            reason: "the MCP namespace 'ah.job.*' is reserved for built-in job tools".to_owned(),
        })
}
