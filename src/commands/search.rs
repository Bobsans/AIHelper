use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse, cancellation,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{cli::GlobalOptions, error::AppError};

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[command(subcommand)]
    pub command: SearchCommand,
}

#[derive(Debug, Subcommand)]
pub enum SearchCommand {
    #[command(about = "Search text in files")]
    Text(TextArgs),
    #[command(about = "Find file paths by substring query")]
    Files(FilesArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextArgs {
    /// Literal text or regular expression to find.
    #[schemars(length(min = 1))]
    pub pattern: String,
    /// Files or directories resolved against context.cwd; empty searches context.cwd.
    #[arg(value_name = "PATH")]
    #[serde(default)]
    #[schemars(default)]
    pub paths: Vec<std::path::PathBuf>,
    /// Optional glob filters relative to each search root.
    #[arg(long = "glob")]
    #[serde(default)]
    pub globs: Vec<String>,
    /// Use case-insensitive matching.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub ignore_case: bool,
    /// Context lines before and after each match.
    #[arg(long)]
    #[serde(rename = "context_lines")]
    pub context: Option<usize>,
    /// Interpret pattern as a regular expression.
    #[arg(
        long,
        help = "Interpret pattern as regex (default: literal/plain search)"
    )]
    #[serde(default)]
    #[schemars(default)]
    pub regex: bool,
    /// Skip files larger than this many bytes.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while scanning"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Follow symlink directories during traversal.
    #[arg(long, help = "Follow symlink directories during traversal")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
}

fn default_max_bytes() -> u64 {
    crate::safety::DEFAULT_MAX_TEXT_BYTES
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilesArgs {
    /// Case-sensitive substring matched against normalized paths; empty lists all files.
    pub query: String,
    /// Files or directories resolved against context.cwd; empty searches context.cwd.
    #[arg(value_name = "PATH")]
    #[serde(default)]
    #[schemars(default)]
    pub paths: Vec<std::path::PathBuf>,
    /// Follow symlink directories during traversal.
    #[arg(long, help = "Follow symlink directories during traversal")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub cwd: Option<PathBuf>,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: SearchArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = match args.command {
        SearchCommand::Text(text_args) => domain::execute_text(text_args, options.limit)?,
        SearchCommand::Files(files_args) => domain::execute_files(files_args, options.limit)?,
    };
    adapters::output::emit(result, options)
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-search",
        "search",
        vec![text_descriptor(), files_descriptor()],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let _cancellation_scope = cancellation::RequestScope::enter(&request.context.request_id);
    if cancellation::is_cancelled() {
        return cancelled_response(request);
    }
    let result = match request.command.as_str() {
        "search.text" => typed_text(request),
        "search.files" => typed_files(request),
        _ => {
            return TypedInvocationResponse::error(CommandError::new(
                Some("search".to_owned()),
                Some(request.command.clone()),
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown search command",
                "the command is not present in the search catalog",
                2,
                false,
            ));
        }
    };
    match result {
        Ok(result) => match result {
            domain::SearchResult::Text(output) => {
                let count = output.match_count;
                typed_success(request, output, format!("Found {count} text match(es)."))
            }
            domain::SearchResult::Files(output) => {
                let count = output.match_count;
                typed_success(request, output, format!("Found {count} matching file(s)."))
            }
        },
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("search")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn cancelled_response(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    TypedInvocationResponse::error(CommandError::new(
        Some("search".to_owned()),
        Some(request.command.clone()),
        "EXECUTION_CANCELLED",
        "Search execution was cancelled",
        format!(
            "request '{}' was cancelled before handler execution",
            request.context.request_id
        ),
        1,
        false,
    ))
}

fn typed_text(request: &TypedInvocationRequest) -> Result<domain::SearchResult, AppError> {
    let mut args: TextArgs = decode(request)?;
    args.cwd = Some(PathBuf::from(&request.context.cwd));
    domain::execute_text(args, request.context.limit)
}

fn typed_files(request: &TypedInvocationRequest) -> Result<domain::SearchResult, AppError> {
    let mut args: FilesArgs = decode(request)?;
    args.cwd = Some(PathBuf::from(&request.context.cwd));
    domain::execute_files(args, request.context.limit)
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

fn typed_success(
    request: &TypedInvocationRequest,
    output: impl serde::Serialize,
    text: String,
) -> TypedInvocationResponse {
    match serde_json::to_value(output) {
        Ok(data) => TypedInvocationResponse::success(data, Some(text)),
        Err(error) => TypedInvocationResponse::error(CommandError::new(
            Some("search".to_owned()),
            Some(request.command.clone()),
            "JSON_SERIALIZATION_FAILED",
            "Failed to serialize search result",
            error.to_string(),
            1,
            false,
        )),
    }
}

fn text_descriptor() -> CommandDescriptor {
    descriptor(
        "search.text",
        "Search file text",
        "Search literal text or a regular expression across files.",
        input_schema_for::<TextArgs>(),
        output_schema_for::<domain::SearchTextOutput>("search.text"),
    )
    .with_example(CommandExample::new(
        "Find TODOs in Rust files",
        json!({"pattern": "TODO", "paths": ["src"], "globs": ["**/*.rs"]}),
    ))
}

fn files_descriptor() -> CommandDescriptor {
    descriptor(
        "search.files",
        "Search file paths",
        "Find normalized file paths containing a substring.",
        input_schema_for::<FilesArgs>(),
        output_schema_for::<domain::SearchFilesOutput>("search.files"),
    )
}

fn descriptor(
    id: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
) -> CommandDescriptor {
    CommandDescriptor::new(
        id,
        title,
        description,
        input_schema,
        output_schema,
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![CommandEffect::FilesystemRead],
            RiskLevel::Medium,
            "Recursively reads file paths and, for text search, file contents below the selected roots. follow_symlinks=true can traverse linked directories outside those roots; no files are modified.",
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
        let request_id = "search-pre-cancelled";
        assert!(cancellation::cancel(request_id));
        let request = TypedInvocationRequest::new(
            "search.files",
            json!({"query": "ignored"}),
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
