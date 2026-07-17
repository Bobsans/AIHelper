use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{cli::GlobalOptions, error::AppError};

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

#[derive(Debug, Args)]
pub struct ProjectPathArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
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
    let output = domain::run_detect(typed_path_args(request))?;
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
    let output = domain::run_commands(typed_path_args(request))?;
    let count = output.commands.len();
    let data = serialized_value(&output)?;
    Ok((data, format!("Suggested {count} project command(s).")))
}

fn typed_version(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let output = domain::run_version(typed_path_args(request), request.context.limit)?;
    let count = output.version_count;
    let data = serialized_value(&output)?;
    Ok((data, format!("Detected {count} project version(s).")))
}

fn typed_path_args(request: &TypedInvocationRequest) -> ProjectPathArgs {
    let raw = request
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or(".");
    let path = PathBuf::from(raw);
    ProjectPathArgs {
        path: if path.is_absolute() {
            path
        } else {
            PathBuf::from(&request.context.cwd).join(path)
        },
    }
}

fn serialized_value<T: Serialize>(output: &T) -> Result<Value, AppError> {
    serde_json::to_value(output).map_err(AppError::from)
}

fn detect_descriptor() -> CommandDescriptor {
    descriptor(
        "project.detect",
        "Detect project",
        "Detect ecosystems, tools, roles, important files, versions, and suggested commands.",
        detect_output_schema(),
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
        top_output(
            "project.commands",
            &[
                ("root", string_schema()),
                ("ecosystems", string_array_schema()),
                ("tools", string_array_schema()),
                ("roles", string_array_schema()),
                (
                    "commands",
                    json!({"type": "array", "items": suggested_command_schema()}),
                ),
            ],
        ),
    )
}

fn version_descriptor() -> CommandDescriptor {
    descriptor(
        "project.version",
        "Detect project versions",
        "Read common manifest files and return detected names and versions.",
        top_output(
            "project.version",
            &[
                ("root", string_schema()),
                ("version_count", nonnegative_integer_schema()),
                ("truncated", json!({"type": "boolean"})),
                (
                    "versions",
                    json!({"type": "array", "items": version_schema()}),
                ),
            ],
        ),
    )
}

fn descriptor(id: &str, title: &str, description: &str, output_schema: Value) -> CommandDescriptor {
    CommandDescriptor::new(
        id,
        title,
        description,
        path_input_schema(),
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

fn path_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "minLength": 1,
                "default": ".",
                "description": "Project path resolved against the execution cwd."
            }
        },
        "additionalProperties": false
    })
}

fn detect_output_schema() -> Value {
    top_output(
        "project.detect",
        &[
            ("root", string_schema()),
            ("ecosystems", string_array_schema()),
            ("tools", string_array_schema()),
            ("roles", string_array_schema()),
            ("files", file_groups_schema()),
            (
                "versions",
                json!({"type": "array", "items": version_schema()}),
            ),
            (
                "commands",
                json!({"type": "array", "items": suggested_command_schema()}),
            ),
            ("package_files", detected_files_schema()),
            ("ci_files", detected_files_schema()),
            ("docs_files", detected_files_schema()),
            ("changelog_files", detected_files_schema()),
        ],
    )
}

fn file_groups_schema() -> Value {
    exact_object(&[
        ("packages", detected_files_schema()),
        ("locks", detected_files_schema()),
        ("ci", detected_files_schema()),
        ("docs", detected_files_schema()),
        ("changelogs", detected_files_schema()),
        ("deploy", detected_files_schema()),
        ("infra", detected_files_schema()),
        ("config", detected_files_schema()),
        ("quality", detected_files_schema()),
        ("security", detected_files_schema()),
    ])
}

fn detected_files_schema() -> Value {
    json!({"type": "array", "items": detected_file_schema()})
}

fn detected_file_schema() -> Value {
    exact_object(&[("kind", string_schema()), ("path", string_schema())])
}

fn version_schema() -> Value {
    exact_object(&[
        ("kind", string_schema()),
        ("path", string_schema()),
        ("name", nullable(string_schema())),
        ("version", nullable(string_schema())),
        ("confidence", string_schema()),
    ])
}

fn suggested_command_schema() -> Value {
    exact_object(&[
        ("kind", string_schema()),
        ("command", string_array_schema()),
        ("confidence", string_schema()),
        ("reason", string_schema()),
    ])
}

fn top_output(command: &str, fields: &[(&str, Value)]) -> Value {
    let mut all_fields = vec![("command", json!({"type": "string", "const": command}))];
    all_fields.extend(fields.iter().cloned());
    exact_object(&all_fields)
}

fn exact_object(fields: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, schema) in fields {
        properties.insert((*name).to_owned(), schema.clone());
        required.push(*name);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn string_array_schema() -> Value {
    json!({"type": "array", "items": string_schema()})
}

fn nonnegative_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 0})
}

fn nullable(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn execute_detect(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_detect(args)?;
    adapters::output::emit_detect(output, options)
}

fn execute_commands(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_commands(args)?;
    adapters::output::emit_commands(output, options)
}

fn execute_version(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_version(args, options.limit)?;
    adapters::output::emit_version(output, options)
}
