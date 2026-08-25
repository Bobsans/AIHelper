use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{cli::GlobalOptions, error::AppError, output::Emitter};

mod rules;

mod adapters {
    pub mod io;
    pub mod output;
}

mod domain;

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    #[command(about = "Detect project ecosystems and important files")]
    Detect(ProjectPathArgs),
    #[command(about = "Suggest common project commands")]
    Commands(ProjectPathArgs),
    #[command(about = "Detect project version from common manifest files")]
    Version(ProjectPathArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectPathArgs {
    /// Project path resolved against the execution cwd.
    #[arg(default_value = ".")]
    #[serde(default = "current_directory")]
    #[schemars(default = "current_directory", extend("minLength" = 1))]
    pub path: PathBuf,
}

fn current_directory() -> PathBuf {
    PathBuf::from(".")
}

pub fn execute(args: ProjectArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        ProjectCommand::Detect(path_args) => execute_detect(path_args, options),
        ProjectCommand::Commands(path_args) => execute_commands(path_args, options),
        ProjectCommand::Version(path_args) => execute_version(path_args, options),
    }
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-project",
        "project",
        vec![
            detect_descriptor(),
            commands_descriptor(),
            version_descriptor(),
        ],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = match request.command.as_str() {
        "project.detect" => typed_detect(request),
        "project.commands" => typed_commands(request),
        "project.version" => typed_version(request),
        _ => {
            return TypedInvocationResponse::error(CommandError::new(
                Some("project".to_owned()),
                Some(request.command.clone()),
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown project command",
                "the command is not present in the project catalog",
                2,
                false,
            ));
        }
    };
    match result {
        Ok((data, text)) => TypedInvocationResponse::success(data, Some(text)),
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("project")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn typed_detect(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let output = domain::run_detect(typed_path_args(request)?)?;
    let data = serialized_value(&output)?;
    let ecosystem_count = output.ecosystems.len();
    let file_count = output.files.packages.len()
        + output.files.locks.len()
        + output.files.ci.len()
        + output.files.docs.len()
        + output.files.changelogs.len()
        + output.files.deploy.len()
        + output.files.infra.len()
        + output.files.config.len()
        + output.files.quality.len()
        + output.files.security.len();
    Ok((
        data,
        format!("Detected {ecosystem_count} ecosystem(s) and {file_count} project file(s)."),
    ))
}

fn typed_commands(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let output = domain::run_commands(typed_path_args(request)?)?;
    let count = output.commands.len();
    let data = serialized_value(&output)?;
    Ok((data, format!("Suggested {count} project command(s).")))
}

fn typed_version(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let output = domain::run_version(typed_path_args(request)?, request.context.limit)?;
    let count = output.version_count;
    let data = serialized_value(&output)?;
    Ok((data, format!("Detected {count} project version(s).")))
}

fn typed_path_args(request: &TypedInvocationRequest) -> Result<ProjectPathArgs, AppError> {
    let mut args: ProjectPathArgs =
        serde_json::from_value(request.arguments.clone()).map_err(|error| {
            AppError::invalid_argument(format!(
                "invalid arguments for {}: {error}",
                request.command
            ))
        })?;
    if !args.path.is_absolute() {
        args.path = PathBuf::from(&request.context.cwd).join(&args.path);
    }
    Ok(args)
}

fn serialized_value<T: Serialize>(output: &T) -> Result<Value, AppError> {
    serde_json::to_value(output).map_err(AppError::from)
}

fn detect_descriptor() -> CommandDescriptor {
    descriptor(
        "project.detect",
        "Detect project",
        "Detect ecosystems, tools, roles, important files, versions, and suggested commands.",
        output_schema_for::<domain::ProjectDetectOutput>("project.detect"),
    )
    .with_example(CommandExample::new(
        "Detect the current project",
        json!({"path": "."}),
    ))
}

fn commands_descriptor() -> CommandDescriptor {
    descriptor(
        "project.commands",
        "Suggest project commands",
        "Suggest common commands from detected manifests and tooling without executing them.",
        output_schema_for::<domain::ProjectCommandsOutput>("project.commands"),
    )
}

fn version_descriptor() -> CommandDescriptor {
    descriptor(
        "project.version",
        "Detect project versions",
        "Read common manifest files and return detected names and versions.",
        output_schema_for::<domain::ProjectVersionOutput>("project.version"),
    )
}

fn descriptor(id: &str, title: &str, description: &str, output_schema: Value) -> CommandDescriptor {
    CommandDescriptor::new(
        id,
        title,
        description,
        input_schema_for::<ProjectPathArgs>(),
        output_schema,
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![CommandEffect::FilesystemRead],
            RiskLevel::Low,
            "Recursively reads recognized project manifests and metadata below the selected path; it does not execute suggested commands or modify files.",
            Reversibility::Yes,
        ),
    )
}

fn execute_detect(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_detect(args)?;
    adapters::output::emit_detect(output, &mut Emitter::stdio(options))
}

fn execute_commands(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_commands(args)?;
    adapters::output::emit_commands(output, &mut Emitter::stdio(options))
}

fn execute_version(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_version(args, options.limit)?;
    adapters::output::emit_version(output, &mut Emitter::stdio(options))
}
