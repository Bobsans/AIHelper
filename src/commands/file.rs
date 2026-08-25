use std::path::{Path, PathBuf};

use crate::{cli::GlobalOptions, error::AppError, output::Emitter};
use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
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

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadArgs {
    /// UTF-8 text file to read.
    #[schemars(extend("minLength" = 1))]
    pub path: std::path::PathBuf,
    /// Prefix returned content lines with source line numbers.
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    #[serde(default)]
    #[schemars(default)]
    pub number_lines: bool,
    /// Inclusive one-based start line.
    #[arg(long, value_name = "N", help = "Start line (1-based)")]
    #[schemars(range(min = 1))]
    pub from: Option<usize>,
    /// Inclusive one-based end line.
    #[arg(long, value_name = "N", help = "End line (1-based)")]
    #[schemars(range(min = 1))]
    pub to: Option<usize>,
    /// Reject a file larger than this byte size.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Allow reading or traversing symlink targets.
    #[arg(long, help = "Allow reading through symlink paths")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
}

fn default_max_bytes() -> u64 {
    crate::safety::DEFAULT_MAX_TEXT_BYTES
}

fn default_lines() -> usize {
    20
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeadArgs {
    /// UTF-8 text file to read.
    #[schemars(extend("minLength" = 1))]
    pub path: std::path::PathBuf,
    /// Number of lines requested.
    #[arg(long, default_value_t = 20)]
    #[serde(default = "default_lines")]
    #[schemars(default = "default_lines")]
    pub lines: usize,
    /// Prefix returned content lines with source line numbers.
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    #[serde(default)]
    #[schemars(default)]
    pub number_lines: bool,
    /// Reject a file larger than this byte size.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Allow reading or traversing symlink targets.
    #[arg(long, help = "Allow reading through symlink paths")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TailArgs {
    /// UTF-8 text file to read.
    #[schemars(extend("minLength" = 1))]
    pub path: std::path::PathBuf,
    /// Number of lines requested.
    #[arg(long, default_value_t = 20)]
    #[serde(default = "default_lines")]
    #[schemars(default = "default_lines")]
    pub lines: usize,
    /// Prefix returned content lines with source line numbers.
    #[arg(short = 'n', long = "number-lines", help = "Show line numbers")]
    #[serde(default)]
    #[schemars(default)]
    pub number_lines: bool,
    /// Reject a file larger than this byte size.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Fail when file size exceeds this limit"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Allow reading or traversing symlink targets.
    #[arg(long, help = "Allow reading through symlink paths")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatArgs {
    /// Filesystem path to inspect.
    #[schemars(extend("minLength" = 1))]
    pub path: std::path::PathBuf,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeArgs {
    /// Tree root; defaults to the context cwd.
    #[schemars(extend("minLength" = 1))]
    pub path: Option<std::path::PathBuf>,
    /// Maximum traversal depth, including zero for the root only.
    #[arg(long)]
    pub depth: Option<usize>,
    /// Allow reading or traversing symlink targets.
    #[arg(long, help = "Follow symlink directories during traversal")]
    #[serde(default)]
    #[schemars(default)]
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
    adapters::output::emit(result, &mut Emitter::stdio(options))
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
    let command = match request.command.as_str() {
        "file.read" => {
            let mut args: ReadArgs = decode(request)?;
            args.path = resolve_context_path(cwd, &args.path);
            FileCommand::Read(args)
        }
        "file.head" => {
            let mut args: HeadArgs = decode(request)?;
            args.path = resolve_context_path(cwd, &args.path);
            FileCommand::Head(args)
        }
        "file.tail" => {
            let mut args: TailArgs = decode(request)?;
            args.path = resolve_context_path(cwd, &args.path);
            FileCommand::Tail(args)
        }
        "file.stat" => {
            let mut args: StatArgs = decode(request)?;
            args.path = resolve_context_path(cwd, &args.path);
            FileCommand::Stat(args)
        }
        "file.tree" => {
            let mut args: TreeArgs = decode(request)?;
            args.path = Some(match args.path.as_deref() {
                Some(path) => resolve_context_path(cwd, path),
                None => cwd.to_path_buf(),
            });
            FileCommand::Tree(args)
        }
        _ => {
            return Err(AppError::invalid_argument(format!(
                "unknown typed file command: {}",
                request.command
            )));
        }
    };
    Ok(FileArgs { command })
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

fn read_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "file.read",
        "Read file lines",
        "Read UTF-8 text from a file with an optional inclusive line range.",
        input_schema_for::<ReadArgs>(),
        output_schema_for::<domain::FileLinesOutput>("file.read"),
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
        input_schema_for::<HeadArgs>(),
        output_schema_for::<domain::FileLinesOutput>("file.head"),
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
        input_schema_for::<TailArgs>(),
        output_schema_for::<domain::FileLinesOutput>("file.tail"),
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
        input_schema_for::<StatArgs>(),
        output_schema_for::<domain::FileStatOutput>("file.stat"),
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
        input_schema_for::<TreeArgs>(),
        output_schema_for::<domain::FileTreeOutput>("file.tree"),
        file_read_effects(
            "Reads directory metadata recursively; enabling symlink following may traverse outside the requested tree.",
        ),
    )
    .with_example(CommandExample::new(
        "List the source tree two levels deep",
        json!({"path": "src", "depth": 2}),
    ))
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
