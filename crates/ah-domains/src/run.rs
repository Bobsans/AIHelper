use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse, cancellation,
    schema::{input_schema_for, output_schema_for},
};
use ah_runtime::RunCheckOutcome;
use serde_json::json;

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;

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

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckArgs {
    /// Process timeout capped by the MCP request deadline.
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(default = "default_timeout_secs")]
    #[schemars(default = "default_timeout_secs", range(min = 1))]
    pub timeout_secs: u64,
    /// Maximum captured bytes for each output stream.
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    #[serde(default = "default_max_output_bytes")]
    #[schemars(default = "default_max_output_bytes", range(min = 1))]
    pub max_output_bytes: usize,
    /// Return only the last N captured lines from each stream.
    #[arg(long, value_name = "N")]
    pub tail_lines: Option<usize>,
    /// Program followed by its arguments; no shell parsing is performed.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    #[schemars(length(min = 1))]
    pub command: Vec<String>,
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

pub mod io;
pub(crate) mod output;
#[cfg(windows)]
pub(crate) mod windows_job;

mod domain;

pub fn execute(args: RunArgs, options: &GlobalOptions) -> Result<(), AppError> {
    execute_observed(args, options).map(|_| ())
}

pub fn execute_observed(
    args: RunArgs,
    options: &GlobalOptions,
) -> Result<RunCheckOutcome, AppError> {
    match args.command {
        RunCommand::Check(mut check_args) => {
            check_args.cwd = options.cwd.clone();
            let result = domain::run_check(check_args)?;
            let outcome = RunCheckOutcome {
                success: result.success,
                timed_out: result.timed_out,
                exit_code: result.exit_code,
            };
            output::emit_check_result(result, &mut Emitter::stdio(options))?;
            Ok(outcome)
        }
    }
}

pub fn command_catalog() -> CommandCatalog {
    CommandCatalog::new("builtin-run", "run", vec![check_descriptor()])
}

pub fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
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
    let _cancellation_scope = cancellation::RequestScope::enter(&request.context.request_id);
    if cancellation::is_cancelled() {
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
    ah_plugin_api::cancellation::cancelled_response(
        "run",
        "Command execution was cancelled",
        request,
    )
}

fn typed_check(request: &TypedInvocationRequest) -> Result<domain::RunCheckOutput, AppError> {
    let mut args: CheckArgs =
        serde_json::from_value(request.arguments.clone()).map_err(|error| {
            AppError::invalid_argument(format!(
                "invalid arguments for {}: {error}",
                request.command
            ))
        })?;
    args.timeout_secs = args.timeout_secs.max(1);
    args.cwd = Some(PathBuf::from(&request.context.cwd));
    args.timeout_ms = Some(
        args.timeout_secs
            .saturating_mul(1_000)
            .min(request.context.remaining_timeout_ms.max(1)),
    );
    domain::run_check(args)
}

fn check_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "run.check",
        "Run command",
        "Run one non-interactive child process, capture bounded output, and return its exit status.",
        input_schema_for::<CheckArgs>(),
        output_schema_for::<domain::RunCheckOutput>("run.check"),
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
        assert!(cancellation::cancel(request_id));
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
        assert!(!cancellation::is_cancelled());
    }
}
