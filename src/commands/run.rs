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
use serde_json::{Value, json};

use crate::{cli::GlobalOptions, error::AppError};
use clap::Args;

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(subcommand)]
    pub command: RunCommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum RunCommand {
    #[command(about = "Run a command and return agent-friendly result")]
    Check(CheckArgs),
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    pub timeout_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    pub max_output_bytes: usize,
    #[arg(long, value_name = "N")]
    pub tail_lines: Option<usize>,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
    #[arg(skip)]
    pub cwd: Option<PathBuf>,
    #[arg(skip)]
    pub timeout_ms: Option<u64>,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: RunArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        RunCommand::Check(check_args) => {
            let result = domain::run_check(check_args)?;
            adapters::output::emit_check_result(result, options)
        }
    }
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
    CommandCatalog::new("builtin-run", "run", vec![check_descriptor()])
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    if request.command != "run.check" {
        return TypedInvocationResponse::error(CommandError::new(
            Some("run".to_owned()),
            Some(request.command.clone()),
            "TYPED_COMMAND_NOT_FOUND",
            "Unknown run command",
            "the command is not present in the run catalog",
            2,
            false,
        ));
    }
    let _cancellation_scope = RequestCancellationScope::enter(request.context.request_id.clone());
    if current_request_cancelled() {
        return cancelled_response(request);
    }
    let response = typed_check(request);

    match response {
        Ok(output) => {
            let success = output.success;
            let exit_code = output.exit_code;
            match serde_json::to_value(output) {
                Ok(data) => TypedInvocationResponse::success(
                    data,
                    Some(format!(
                        "Command completed with success={success} and exit_code={exit_code:?}."
                    )),
                ),
                Err(error) => TypedInvocationResponse::error(CommandError::new(
                    Some("run".to_owned()),
                    Some(request.command.clone()),
                    "JSON_SERIALIZATION_FAILED",
                    "Failed to serialize command result",
                    error.to_string(),
                    1,
                    false,
                )),
            }
        }
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("run")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn cancelled_response(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    TypedInvocationResponse::error(CommandError::new(
        Some("run".to_owned()),
        Some(request.command.clone()),
        "EXECUTION_CANCELLED",
        "Command execution was cancelled",
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

fn typed_check(request: &TypedInvocationRequest) -> Result<domain::RunCheckOutput, AppError> {
    let command = request
        .arguments
        .get("command")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let requested_timeout_secs = request
        .arguments
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .max(1);
    let timeout_ms = requested_timeout_secs
        .saturating_mul(1_000)
        .min(request.context.remaining_timeout_ms.max(1));
    let max_output_bytes = request
        .arguments
        .get("max_output_bytes")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
    let tail_lines = request
        .arguments
        .get("tail_lines")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok());
    domain::run_check(CheckArgs {
        timeout_secs: requested_timeout_secs,
        max_output_bytes,
        tail_lines,
        command,
        cwd: Some(PathBuf::from(&request.context.cwd)),
        timeout_ms: Some(timeout_ms),
    })
}

fn check_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "run.check",
        "Run command",
        "Run one non-interactive child process, capture bounded output, and return its exit status.",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "string"},
                    "description": "Program followed by its arguments; no shell parsing is performed."
                },
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
                    "description": "Maximum captured bytes for each output stream."
                },
                "tail_lines": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Return only the last N captured lines from each stream."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "run.check"},
                "argv": {"type": "array", "items": {"type": "string"}},
                "success": {"type": "boolean"},
                "timed_out": {"type": "boolean"},
                "exit_code": {
                    "oneOf": [
                        {"type": "integer"},
                        {"type": "null"}
                    ]
                },
                "duration_ms": {"type": "integer", "minimum": 0},
                "stdout": {"type": "string"},
                "stderr": {"type": "string"},
                "stdout_truncated": {"type": "boolean"},
                "stderr_truncated": {"type": "boolean"}
            },
            "required": [
                "command",
                "argv",
                "success",
                "timed_out",
                "exit_code",
                "duration_ms",
                "stdout",
                "stderr",
                "stdout_truncated",
                "stderr_truncated"
            ],
            "additionalProperties": false
        }),
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
            "Executes an arbitrary program in context.cwd with the server environment. The child can read, modify, or delete files, access the network, change external systems, and expose inherited secrets. Stdin is closed; timeout or cancellation terminates its process group.",
            Reversibility::Unknown,
        ),
    )
    .with_example(CommandExample::new(
        "Run a Rust check",
        json!({"command": ["cargo", "check"], "timeout_secs": 120}),
    ))
}

#[cfg(test)]
mod cancellation_tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "run-pre-cancelled";
        assert!(cancel_typed(request_id));
        let request = TypedInvocationRequest::new(
            "run.check",
            json!({"command": ["ignored"]}),
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

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn resolves_extensionless_program_from_path_with_pathext_order() {
        let temp_dir = tempfile::tempdir().expect("temp dir should be created");
        let shim = temp_dir.path().join("npx.CMD");
        fs::write(&shim, "@echo off\r\n").expect("shim should be written");

        let resolved = resolve_windows_program_in(
            "npx",
            None,
            Some(&[temp_dir.path().to_path_buf()]),
            &[".EXE".to_owned(), ".CMD".to_owned()],
        )
        .expect("npx should resolve through PATHEXT");

        assert_eq!(resolved, shim);
    }

    #[test]
    fn does_not_rewrite_programs_that_already_have_an_extension() {
        assert!(resolve_windows_program("npx.cmd").is_none());
    }
}

#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn resolve_windows_program_in(
    program: &str,
    current_dir: Option<&std::path::Path>,
    path_dirs: Option<&[std::path::PathBuf]>,
    path_exts: &[String],
) -> Option<std::path::PathBuf> {
    adapters::io::resolve_windows_program_in(program, current_dir, path_dirs, path_exts)
}

#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn resolve_windows_program(program: &str) -> Option<std::path::PathBuf> {
    adapters::io::resolve_windows_program(program)
}
