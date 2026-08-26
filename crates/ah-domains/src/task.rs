use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse, cancellation,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions};

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const TASK_NAME_PATTERN: &str = "^[A-Za-z0-9._-]+$";

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

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaveArgs {
    /// Task name.
    #[schemars(length(min = 1), regex(pattern = TASK_NAME_PATTERN))]
    pub name: String,
    /// Shell command stored verbatim; it is not executed by this tool.
    #[schemars(length(min = 1))]
    pub command: String,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunArgs {
    /// Task name.
    #[schemars(length(min = 1), regex(pattern = TASK_NAME_PATTERN))]
    pub name: String,
    /// Process timeout capped by the MCP request deadline.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(default = "default_timeout_secs")]
    #[schemars(default = "default_timeout_secs", range(min = 1))]
    pub timeout_secs: u64,
    /// Maximum captured bytes per output stream.
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    #[serde(default = "default_max_output_bytes")]
    #[schemars(default = "default_max_output_bytes", range(min = 1))]
    pub max_output_bytes: usize,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
    #[arg(skip)]
    #[serde(skip)]
    pub timeout_ms: Option<u64>,
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_max_output_bytes() -> usize {
    DEFAULT_MAX_OUTPUT_BYTES
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListArgs {
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
}

pub mod io;
pub(crate) mod output;

mod domain;

pub fn execute(mut args: TaskArgs, options: &GlobalOptions) -> Result<(), AppError> {
    if options.cwd.is_some() {
        set_cwd(&mut args.command, options.cwd.clone());
    }
    let result = domain::execute(args, options.limit)?;
    output::emit(result, &mut Emitter::stdio(options))
}

/// Point every task command at the directory the request named; the task store
/// lives under it.
///
/// Shared by both entry points: the CLI used to get this by the process having
/// been `chdir`-ed, which is the same answer only as long as one request is in
/// flight at a time.
fn set_cwd(command: &mut TaskCommand, cwd: Option<PathBuf>) {
    match command {
        TaskCommand::Save(args) => args.cwd = cwd,
        TaskCommand::Run(args) => args.cwd = cwd,
        TaskCommand::List(args) => args.cwd = cwd,
    }
}

pub fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-task",
        "task",
        vec![save_descriptor(), run_descriptor(), list_descriptor()],
    )
}

pub fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let _cancellation_scope = cancellation::RequestScope::enter(&request.context.request_id);
    if cancellation::is_cancelled() {
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

fn typed_execute(request: &TypedInvocationRequest) -> Result<domain::TaskResult, AppError> {
    let cwd = Some(PathBuf::from(&request.context.cwd));
    let command = match request.command.as_str() {
        "task.save" => {
            let mut args: SaveArgs = decode(request)?;
            args.cwd = cwd;
            TaskCommand::Save(args)
        }
        "task.run" => {
            let mut args: RunArgs = decode(request)?;
            args.timeout_secs = args.timeout_secs.max(1);
            args.cwd = cwd;
            args.timeout_ms = Some(
                args.timeout_secs
                    .saturating_mul(1_000)
                    .min(request.context.remaining_timeout_ms.max(1)),
            );
            TaskCommand::Run(args)
        }
        "task.list" => {
            let mut args: ListArgs = decode(request)?;
            args.cwd = cwd;
            TaskCommand::List(args)
        }
        _ => {
            return Err(AppError::invalid_argument(format!(
                "unknown typed task command: {}",
                request.command
            )));
        }
    };
    domain::execute(TaskArgs { command }, request.context.limit)
}

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the type disagree.
fn decode<T: serde::de::DeserializeOwned>(request: &TypedInvocationRequest) -> Result<T, AppError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        AppError::invalid_argument(format!(
            "invalid arguments for {}: {error}",
            request.command
        ))
    })
}

fn save_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "task.save",
        "Save task",
        "Create or replace a named shell command in context.cwd/.ah/tasks.json.",
        input_schema_for::<SaveArgs>(),
        output_schema_for::<domain::TaskSaveOutput>("task.save"),
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
        input_schema_for::<RunArgs>(),
        output_schema_for::<domain::TaskRunOutput>("task.run"),
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
        input_schema_for::<ListArgs>(),
        output_schema_for::<domain::TaskListOutput>("task.list"),
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

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "task-pre-cancelled";
        assert!(cancellation::cancel(request_id));
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
        assert!(!cancellation::is_cancelled());
    }
}
