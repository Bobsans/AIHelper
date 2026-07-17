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
use serde_json::{Map, Value, json};

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

#[derive(Debug, Args)]
pub struct TextArgs {
    pub pattern: String,
    #[arg(value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
    #[arg(long = "glob")]
    pub globs: Vec<String>,
    #[arg(long)]
    pub ignore_case: bool,
    #[arg(long)]
    pub context: Option<usize>,
    #[arg(
        long,
        help = "Interpret pattern as regex (default: literal/plain search)"
    )]
    pub regex: bool,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while scanning"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
    #[arg(skip)]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct FilesArgs {
    pub query: String,
    #[arg(value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
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

pub fn execute(args: SearchArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = match args.command {
        SearchCommand::Text(text_args) => domain::execute_text(text_args, options.limit)?,
        SearchCommand::Files(files_args) => domain::execute_files(files_args, options.limit)?,
    };
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
        "builtin-search",
        "search",
        vec![text_descriptor(), files_descriptor()],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let _cancellation_scope = RequestCancellationScope::enter(request.context.request_id.clone());
    if current_request_cancelled() {
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

fn typed_text(request: &TypedInvocationRequest) -> Result<domain::SearchResult, AppError> {
    let arguments = &request.arguments;
    domain::execute_text(
        TextArgs {
            pattern: arguments
                .get("pattern")
                .and_then(Value::as_str)
                .expect("validated search.text input contains pattern")
                .to_owned(),
            paths: typed_paths(arguments),
            globs: string_array(arguments, "globs"),
            ignore_case: typed_bool(arguments, "ignore_case"),
            context: arguments
                .get("context_lines")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            regex: typed_bool(arguments, "regex"),
            max_bytes: arguments
                .get("max_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(crate::safety::DEFAULT_MAX_TEXT_BYTES),
            follow_symlinks: typed_bool(arguments, "follow_symlinks"),
            cwd: Some(PathBuf::from(&request.context.cwd)),
        },
        request.context.limit,
    )
}

fn typed_files(request: &TypedInvocationRequest) -> Result<domain::SearchResult, AppError> {
    let arguments = &request.arguments;
    domain::execute_files(
        FilesArgs {
            query: arguments
                .get("query")
                .and_then(Value::as_str)
                .expect("validated search.files input contains query")
                .to_owned(),
            paths: typed_paths(arguments),
            follow_symlinks: typed_bool(arguments, "follow_symlinks"),
            cwd: Some(PathBuf::from(&request.context.cwd)),
        },
        request.context.limit,
    )
}

fn typed_paths(arguments: &Value) -> Vec<PathBuf> {
    arguments
        .get("paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn string_array(arguments: &Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn typed_bool(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
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
    let mut properties = common_properties();
    properties.insert(
        "pattern".to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "description": "Literal text or regular expression to find."
        }),
    );
    properties.insert(
        "globs".to_owned(),
        json!({
            "type": "array",
            "items": {"type": "string"},
            "description": "Optional glob filters relative to each search root."
        }),
    );
    properties.insert(
        "ignore_case".to_owned(),
        boolean_schema(false, "Use case-insensitive matching."),
    );
    properties.insert(
        "context_lines".to_owned(),
        json!({
            "type": "integer",
            "minimum": 0,
            "description": "Context lines before and after each match."
        }),
    );
    properties.insert(
        "regex".to_owned(),
        boolean_schema(false, "Interpret pattern as a regular expression."),
    );
    properties.insert(
        "max_bytes".to_owned(),
        json!({
            "type": "integer",
            "minimum": 1,
            "default": crate::safety::DEFAULT_MAX_TEXT_BYTES,
            "description": "Skip files larger than this many bytes."
        }),
    );
    descriptor(
        "search.text",
        "Search file text",
        "Search literal text or a regular expression across files.",
        object_schema(properties, vec!["pattern"]),
        text_output_schema(),
    )
    .with_example(CommandExample::new(
        "Find TODOs in Rust files",
        json!({"pattern": "TODO", "paths": ["src"], "globs": ["**/*.rs"]}),
    ))
}

fn files_descriptor() -> CommandDescriptor {
    let mut properties = common_properties();
    properties.insert(
        "query".to_owned(),
        json!({
            "type": "string",
            "description": "Case-sensitive substring matched against normalized paths; empty lists all files."
        }),
    );
    descriptor(
        "search.files",
        "Search file paths",
        "Find normalized file paths containing a substring.",
        object_schema(properties, vec!["query"]),
        files_output_schema(),
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

fn common_properties() -> Map<String, Value> {
    Map::from_iter([
        (
            "paths".to_owned(),
            json!({
                "type": "array",
                "items": {"type": "string"},
                "default": [],
                "description": "Files or directories resolved against context.cwd; empty searches context.cwd."
            }),
        ),
        (
            "follow_symlinks".to_owned(),
            boolean_schema(false, "Follow symlink directories during traversal."),
        ),
    ])
}

fn object_schema(properties: Map<String, Value>, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn boolean_schema(default: bool, description: &str) -> Value {
    json!({"type": "boolean", "default": default, "description": description})
}

fn text_output_schema() -> Value {
    top_output(
        "search.text",
        &[
            ("backend", string_schema()),
            ("root", string_schema()),
            ("roots", string_array_schema()),
            ("pattern", string_schema()),
            ("regex", boolean_value_schema()),
            ("ignore_case", boolean_value_schema()),
            ("context", nonnegative_integer_schema()),
            ("match_count", nonnegative_integer_schema()),
            ("file_count", nonnegative_integer_schema()),
            ("skipped_binary_files", nonnegative_integer_schema()),
            ("skipped_large_files", nonnegative_integer_schema()),
            ("skipped_symlink_files", nonnegative_integer_schema()),
            ("truncated", boolean_value_schema()),
            (
                "matches",
                json!({"type": "array", "items": text_match_schema()}),
            ),
        ],
    )
}

fn files_output_schema() -> Value {
    top_output(
        "search.files",
        &[
            ("backend", string_schema()),
            ("root", string_schema()),
            ("roots", string_array_schema()),
            ("query", string_schema()),
            ("match_count", nonnegative_integer_schema()),
            ("truncated", boolean_value_schema()),
            ("files", string_array_schema()),
        ],
    )
}

fn text_match_schema() -> Value {
    exact_object(&[
        ("path", string_schema()),
        ("line", positive_integer_schema()),
        ("column", positive_integer_schema()),
        ("text", string_schema()),
        (
            "context_before",
            json!({"type": "array", "items": context_line_schema()}),
        ),
        (
            "context_after",
            json!({"type": "array", "items": context_line_schema()}),
        ),
    ])
}

fn context_line_schema() -> Value {
    exact_object(&[
        ("line", positive_integer_schema()),
        ("text", string_schema()),
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

fn boolean_value_schema() -> Value {
    json!({"type": "boolean"})
}

fn positive_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 1})
}

fn nonnegative_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 0})
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn cancellation_delivered_before_handler_entry_is_preserved() {
        let request_id = "search-pre-cancelled";
        assert!(cancel_typed(request_id));
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
        assert!(!cancellation_requests().lock().unwrap().contains(request_id));
    }
}
