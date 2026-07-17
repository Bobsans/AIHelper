use std::{
    cell::RefCell,
    collections::HashSet,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use crate::{cli::GlobalOptions, error::AppError};

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    #[command(about = "Save a reusable shell command")]
    Save(SaveArgs),
    #[command(about = "Run a saved task by name")]
    Run(RunArgs),
    #[command(about = "List saved tasks")]
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct SaveArgs {
    pub name: String,
    pub command: String,
    #[arg(skip)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    pub name: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    pub timeout_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    pub max_output_bytes: usize,
    #[arg(skip)]
    pub cwd: Option<PathBuf>,
    #[arg(skip)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    #[arg(skip)]
    pub cwd: Option<PathBuf>,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: TaskArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = domain::execute(args, options.limit)?;
    adapters::output::emit(result, options)
}

thread_local! {
    static CURRENT_REQUEST_ID: RefCell<Option<String>> = const { RefCell::new(None) };
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
        cancellation_requests()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.request_id);
    }
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-task",
        "task",
        vec![save_descriptor(), run_descriptor(), list_descriptor()],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let _cancellation_scope = RequestCancellationScope::enter(request.context.request_id.clone());
    if current_request_cancelled() {
        return cancelled_response(request);
    }
    let result = typed_execute(request);
    match result {
        Ok(result) => {
            let (data, text) = match result {
                domain::TaskResult::Save(output) => {
                    let text = format!("Saved task '{}'.", output.name);
                    (serde_json::to_value(output), text)
                }
                domain::TaskResult::List(output) => {
                    let text = format!("Returned {} saved task(s).", output.count);
                    (serde_json::to_value(output), text)
                }
                domain::TaskResult::Run(output) => {
                    let text = format!("Task '{}' completed successfully.", output.name);
                    (serde_json::to_value(output), text)
                }
            };
            match data {
                Ok(data) => TypedInvocationResponse::success(data, Some(text)),
                Err(error) => TypedInvocationResponse::error(CommandError::new(
                    Some("task".to_owned()),
                    Some(request.command.clone()),
                    "JSON_SERIALIZATION_FAILED",
                    "Failed to serialize task result",
                    error.to_string(),
                    1,
                    false,
                )),
            }
        }
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("task")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn cancelled_response(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    TypedInvocationResponse::error(CommandError::new(
        Some("task".to_owned()),
        Some(request.command.clone()),
        "EXECUTION_CANCELLED",
        "Task execution was cancelled",
        format!(
            "request '{}' was cancelled before handler execution",
            request.context.request_id
        ),
        1,
        false,
    ))
}

pub(crate) fn cancel_typed(request_id: &str) -> bool {
    cancellation_requests()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(request_id.to_owned());
    true
}

pub(crate) fn current_request_cancelled() -> bool {
    let Some(request_id) = CURRENT_REQUEST_ID.with(|current| current.borrow().clone()) else {
        return false;
    };
    cancellation_requests()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(&request_id)
}

fn cancellation_requests() -> &'static Mutex<HashSet<String>> {
    static REQUESTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn typed_execute(request: &TypedInvocationRequest) -> Result<domain::TaskResult, AppError> {
    let arguments = &request.arguments;
    let cwd = Some(PathBuf::from(&request.context.cwd));
    let command = match request.command.as_str() {
        "task.save" => TaskCommand::Save(SaveArgs {
            name: required_string(arguments, "name"),
            command: required_string(arguments, "command"),
            cwd,
        }),
        "task.run" => {
            let timeout_secs = arguments
                .get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_TIMEOUT_SECS)
                .max(1);
            TaskCommand::Run(RunArgs {
                name: required_string(arguments, "name"),
                timeout_secs,
                max_output_bytes: arguments
                    .get("max_output_bytes")
                    .and_then(Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok())
                    .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES),
                cwd,
                timeout_ms: Some(
                    timeout_secs
                        .saturating_mul(1_000)
                        .min(request.context.remaining_timeout_ms.max(1)),
                ),
            })
        }
        "task.list" => TaskCommand::List(ListArgs { cwd }),
        _ => {
            return Err(AppError::invalid_argument(format!(
                "unknown typed task command: {}",
                request.command
            )));
        }
    };
    domain::execute(TaskArgs { command }, request.context.limit)
}

fn required_string(arguments: &Value, name: &str) -> String {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .expect("validated task input contains required string")
        .to_owned()
}

fn save_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "task.save",
        "Save task",
        "Create or replace a named shell command in context.cwd/.ah/tasks.json.",
        json!({
            "type": "object",
            "properties": {
                "name": task_name_schema(),
                "command": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Shell command stored verbatim; it is not executed by this tool."
                }
            },
            "required": ["name", "command"],
            "additionalProperties": false
        }),
        save_output_schema(),
        CommandEffects::new(
            false,
            true,
            false,
            false,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::FilesystemWrite,
                CommandEffect::ConfigurationWrite,
            ],
            RiskLevel::High,
            "Creates or rewrites context.cwd/.ah/tasks.json and may replace an existing task definition shared by later invocations. The command is stored but not executed.",
            Reversibility::Unknown,
        ),
    )
    .with_example(CommandExample::new(
        "Save a test task",
        json!({"name": "test", "command": "cargo test --workspace"}),
    ))
}

fn run_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "task.run",
        "Run saved task",
        "Load a named task and execute its command through the platform shell.",
        json!({
            "type": "object",
            "properties": {
                "name": task_name_schema(),
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_TIMEOUT_SECS,
                    "description": "Process timeout capped by the MCP request deadline."
                },
                "max_output_bytes": {
                    "type": "integer",
                    "minimum": 1,
                    "default": DEFAULT_MAX_OUTPUT_BYTES,
                    "description": "Maximum captured bytes per output stream."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        run_output_schema(),
        CommandEffects::new(
            false,
            true,
            false,
            true,
            vec![
                CommandEffect::ProcessSpawn,
                CommandEffect::FilesystemRead,
                CommandEffect::FilesystemWrite,
                CommandEffect::FilesystemDelete,
                CommandEffect::NetworkRead,
                CommandEffect::NetworkWrite,
                CommandEffect::ConfigurationRead,
                CommandEffect::ConfigurationWrite,
                CommandEffect::ExternalRead,
                CommandEffect::ExternalWrite,
            ],
            RiskLevel::Critical,
            "Executes the saved text through PowerShell or sh in context.cwd with the server environment. Shell expansion can run arbitrary programs, read or change files, access networks and external systems, and expose inherited secrets. Timeout or cancellation terminates the process group.",
            Reversibility::Unknown,
        ),
    )
}

fn list_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "task.list",
        "List saved tasks",
        "Read saved task definitions from context.cwd/.ah/tasks.json.",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        list_output_schema(),
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::ConfigurationRead,
            ],
            RiskLevel::Medium,
            "Reads and returns every saved shell command in context.cwd/.ah/tasks.json; commands may contain sensitive arguments.",
            Reversibility::Yes,
        ),
    )
}

fn task_name_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "pattern": "^[A-Za-z0-9._-]+$",
        "description": "Task name."
    })
}

fn save_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "task.save"},
            "name": {"type": "string"},
            "task_command": {"type": "string"},
            "store_path": {"type": "string"},
            "updated_unix_seconds": {"type": "integer", "minimum": 0}
        },
        "required": [
            "command",
            "name",
            "task_command",
            "store_path",
            "updated_unix_seconds"
        ],
        "additionalProperties": false
    })
}

fn list_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "task.list"},
            "store_path": {"type": "string"},
            "count": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "tasks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "command": {"type": "string"},
                        "updated_unix_seconds": {"type": "integer", "minimum": 0}
                    },
                    "required": ["name", "command", "updated_unix_seconds"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["command", "store_path", "count", "truncated", "tasks"],
        "additionalProperties": false
    })
}

fn run_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "task.run"},
            "name": {"type": "string"},
            "task_command": {"type": "string"},
            "exit_code": {"type": "integer"},
            "success": {"type": "boolean"},
            "truncated": {"type": "boolean"},
            "stdout": {"type": "string"},
            "stderr": {"type": "string"}
        },
        "required": [
            "command",
            "name",
            "task_command",
            "exit_code",
            "success",
            "truncated",
            "stdout",
            "stderr"
        ],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "task-pre-cancelled";
        assert!(cancel_typed(request_id));
        let request = TypedInvocationRequest::new(
            "task.list",
            json!({}),
            ExecutionContextWire::new(request_id, ".", None, 1_000),
        );

        let response = invoke_typed(&request);

        assert!(!response.success);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("EXECUTION_CANCELLED")
        );
        assert!(!cancellation_requests().lock().unwrap().contains(request_id));
    }
}
