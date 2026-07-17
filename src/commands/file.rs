use std::path::{Path, PathBuf};

use crate::{cli::GlobalOptions, error::AppError};
use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

#[derive(Debug, Args)]
pub struct FileArgs {
    #[command(subcommand)]
    pub command: FileCommand,
}

#[derive(Debug, Subcommand)]
pub enum FileCommand {
    #[command(about = "Read file content (supports line range and numbering)")]
    Read(ReadArgs),
    #[command(about = "Show first N lines of a file")]
    Head(HeadArgs),
    #[command(about = "Show last N lines of a file")]
    Tail(TailArgs),
    #[command(about = "Show file metadata")]
    Stat(StatArgs),
    #[command(about = "Show directory tree")]
    Tree(TreeArgs),
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    pub path: std::path::PathBuf,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
    #[arg(long, value_name = "N", help = "Start line (1-based)")]
    pub from: Option<usize>,
    #[arg(long, value_name = "N", help = "End line (1-based)")]
    pub to: Option<usize>,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Allow reading through symlink paths")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct HeadArgs {
    pub path: std::path::PathBuf,
    #[arg(long, default_value_t = 20)]
    pub lines: usize,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Allow reading through symlink paths")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct TailArgs {
    pub path: std::path::PathBuf,
    #[arg(long, default_value_t = 20)]
    pub lines: usize,
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    pub number_lines: bool,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Allow reading through symlink paths")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct StatArgs {
    pub path: std::path::PathBuf,
}

#[derive(Debug, Args)]
pub struct TreeArgs {
    pub path: Option<std::path::PathBuf>,
    #[arg(long)]
    pub depth: Option<usize>,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: FileArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = domain::execute(args, options.limit)?;
    adapters::output::emit(result, options)
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-file",
        "file",
        vec![
            read_descriptor(),
            head_descriptor(),
            tail_descriptor(),
            stat_descriptor(),
            tree_descriptor(),
        ],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = typed_args(request)
        .and_then(|args| domain::execute(args, request.context.limit))
        .and_then(result_to_value);
    match result {
        Ok(data) => {
            let text = file_result_text(&request.command, &data);
            TypedInvocationResponse::success(data, Some(text))
        }
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("file")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn typed_args(request: &TypedInvocationRequest) -> Result<FileArgs, AppError> {
    let cwd = Path::new(&request.context.cwd);
    let path = |required: bool| -> Result<Option<PathBuf>, AppError> {
        match request.arguments.get("path").and_then(Value::as_str) {
            Some(path) => Ok(Some(resolve_context_path(cwd, Path::new(path)))),
            None if required => Err(AppError::invalid_argument("missing file path")),
            None => Ok(None),
        }
    };
    let command = match request.command.as_str() {
        "file.read" => FileCommand::Read(ReadArgs {
            path: path(true)?.expect("required path should exist"),
            number_lines: typed_bool(&request.arguments, "number_lines"),
            from: typed_usize(&request.arguments, "from"),
            to: typed_usize(&request.arguments, "to"),
            max_bytes: typed_max_bytes(&request.arguments),
            follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
        }),
        "file.head" => FileCommand::Head(HeadArgs {
            path: path(true)?.expect("required path should exist"),
            lines: typed_usize(&request.arguments, "lines").unwrap_or(20),
            number_lines: typed_bool(&request.arguments, "number_lines"),
            max_bytes: typed_max_bytes(&request.arguments),
            follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
        }),
        "file.tail" => FileCommand::Tail(TailArgs {
            path: path(true)?.expect("required path should exist"),
            lines: typed_usize(&request.arguments, "lines").unwrap_or(20),
            number_lines: typed_bool(&request.arguments, "number_lines"),
            max_bytes: typed_max_bytes(&request.arguments),
            follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
        }),
        "file.stat" => FileCommand::Stat(StatArgs {
            path: path(true)?.expect("required path should exist"),
        }),
        "file.tree" => FileCommand::Tree(TreeArgs {
            path: path(false)?.or_else(|| Some(cwd.to_path_buf())),
            depth: typed_usize(&request.arguments, "depth"),
            follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
        }),
        _ => {
            return Err(AppError::invalid_argument(format!(
                "unknown typed file command: {}",
                request.command
            )));
        }
    };
    Ok(FileArgs { command })
}

fn result_to_value(result: domain::FileResult) -> Result<Value, AppError> {
    Ok(match result {
        domain::FileResult::Read(value)
        | domain::FileResult::Head(value)
        | domain::FileResult::Tail(value) => serde_json::to_value(value)?,
        domain::FileResult::Stat(value) => serde_json::to_value(value)?,
        domain::FileResult::Tree(value) => serde_json::to_value(value)?,
    })
}

fn file_result_text(command: &str, data: &Value) -> String {
    match command {
        "file.read" | "file.head" | "file.tail" => format!(
            "Returned {} line(s) from {}.",
            data["line_count"].as_u64().unwrap_or(0),
            data["path"].as_str().unwrap_or("the file")
        ),
        "file.stat" => format!(
            "Returned metadata for {}.",
            data["path"].as_str().unwrap_or("the path")
        ),
        "file.tree" => format!(
            "Returned {} tree entry or entries.",
            data["entry_count"].as_u64().unwrap_or(0)
        ),
        _ => "Returned file data.".to_owned(),
    }
}

fn resolve_context_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn typed_bool(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn typed_usize(arguments: &Value, name: &str) -> Option<usize> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn typed_max_bytes(arguments: &Value) -> u64 {
    arguments
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(crate::safety::DEFAULT_MAX_TEXT_BYTES)
}

fn read_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.read",
        "Read file lines",
        "Read UTF-8 text from a file with an optional inclusive line range.",
        line_input_schema(false),
        lines_output_schema("file.read"),
        file_read_effects(
            "Reads the requested file; enabling symlink following may read a target outside its apparent path.",
        ),
    )
    .with_example(CommandExample::new(
        "Read the first 120 numbered lines",
        json!({"path": "src/main.rs", "number_lines": true, "from": 1, "to": 120}),
    ))
}

fn head_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.head",
        "Read file head",
        "Read the first requested number of UTF-8 text lines from a file.",
        line_input_schema(true),
        lines_output_schema("file.head"),
        file_read_effects(
            "Reads the beginning of the requested file; enabling symlink following may read an external target.",
        ),
    )
    .with_example(CommandExample::new(
        "Read the first 40 numbered lines",
        json!({"path": "src/lib.rs", "lines": 40, "number_lines": true}),
    ))
}

fn tail_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.tail",
        "Read file tail",
        "Read the last requested number of UTF-8 text lines from a file.",
        line_input_schema(true),
        lines_output_schema("file.tail"),
        file_read_effects(
            "Reads the requested file to determine its final lines; enabling symlink following may read an external target.",
        ),
    )
    .with_example(CommandExample::new(
        "Read the last 30 lines",
        json!({"path": "CHANGELOG.md", "lines": 30}),
    ))
}

fn stat_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.stat",
        "Inspect file metadata",
        "Return filesystem metadata for one file, directory, symlink, or other path.",
        path_only_input_schema(),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "file.stat"},
                "path": {"type": "string"},
                "kind": {
                    "type": "string",
                    "enum": ["file", "directory", "symlink", "other"]
                },
                "size_bytes": {"type": "integer", "minimum": 0},
                "readonly": {"type": "boolean"},
                "modified_unix_seconds": {"type": ["integer", "null"], "minimum": 0},
                "created_unix_seconds": {"type": ["integer", "null"], "minimum": 0}
            },
            "required": [
                "command",
                "path",
                "kind",
                "size_bytes",
                "readonly",
                "modified_unix_seconds",
                "created_unix_seconds"
            ],
            "additionalProperties": false
        }),
        file_read_effects("Reads filesystem metadata for the requested path only."),
    )
    .with_example(CommandExample::new(
        "Inspect Cargo.toml metadata",
        json!({"path": "Cargo.toml"}),
    ))
}

fn tree_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.tree",
        "List directory tree",
        "Return a deterministic directory tree with optional depth and output limits.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Tree root; defaults to the context cwd."
                },
                "depth": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Maximum traversal depth, including zero for the root only."
                },
                "follow_symlinks": follow_symlinks_schema()
            },
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "file.tree"},
                "path": {"type": "string"},
                "max_depth": {"type": ["integer", "null"], "minimum": 0},
                "entry_count": {"type": "integer", "minimum": 0},
                "truncated": {"type": "boolean"},
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "depth": {"type": "integer", "minimum": 0},
                            "kind": {
                                "type": "string",
                                "enum": ["file", "directory", "symlink", "other"]
                            },
                            "name": {"type": "string"},
                            "path": {"type": "string"}
                        },
                        "required": ["depth", "kind", "name", "path"],
                        "additionalProperties": false
                    }
                }
            },
            "required": [
                "command",
                "path",
                "max_depth",
                "entry_count",
                "truncated",
                "entries"
            ],
            "additionalProperties": false
        }),
        file_read_effects(
            "Reads directory metadata recursively; enabling symlink following may traverse outside the requested tree.",
        ),
    )
    .with_example(CommandExample::new(
        "List the source tree two levels deep",
        json!({"path": "src", "depth": 2}),
    ))
}

fn path_only_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "minLength": 1,
                "description": "Filesystem path to inspect."
            }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

fn line_input_schema(head_or_tail: bool) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "path".to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "description": "UTF-8 text file to read."
        }),
    );
    if head_or_tail {
        properties.insert(
            "lines".to_owned(),
            json!({
                "type": "integer",
                "minimum": 0,
                "default": 20,
                "description": "Number of lines requested."
            }),
        );
    } else {
        properties.insert(
            "from".to_owned(),
            json!({
                "type": "integer",
                "minimum": 1,
                "description": "Inclusive one-based start line."
            }),
        );
        properties.insert(
            "to".to_owned(),
            json!({
                "type": "integer",
                "minimum": 1,
                "description": "Inclusive one-based end line."
            }),
        );
    }
    properties.insert(
        "number_lines".to_owned(),
        json!({
            "type": "boolean",
            "default": false,
            "description": "Prefix returned content lines with source line numbers."
        }),
    );
    properties.insert("max_bytes".to_owned(), max_bytes_schema());
    properties.insert("follow_symlinks".to_owned(), follow_symlinks_schema());
    json!({
        "type": "object",
        "properties": properties,
        "required": ["path"],
        "additionalProperties": false
    })
}

fn max_bytes_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": crate::safety::DEFAULT_MAX_TEXT_BYTES,
        "description": "Reject a file larger than this byte size."
    })
}

fn follow_symlinks_schema() -> Value {
    json!({
        "type": "boolean",
        "default": false,
        "description": "Allow reading or traversing symlink targets."
    })
}

fn lines_output_schema(command: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": command},
            "path": {"type": "string"},
            "from": {"type": ["integer", "null"], "minimum": 1},
            "to": {"type": ["integer", "null"], "minimum": 1},
            "numbered": {"type": "boolean"},
            "line_count": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "content": {"type": "string"}
        },
        "required": [
            "command",
            "path",
            "from",
            "to",
            "numbered",
            "line_count",
            "truncated",
            "content"
        ],
        "additionalProperties": false
    })
}

fn file_read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        false,
        vec![CommandEffect::FilesystemRead],
        RiskLevel::Low,
        impact,
        Reversibility::Yes,
    )
}
