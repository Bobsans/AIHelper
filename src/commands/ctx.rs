use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::{cli::GlobalOptions, error::AppError};

#[derive(Debug, Args)]
pub struct CtxArgs {
    #[command(subcommand)]
    pub command: CtxCommand,
}

#[derive(Debug, Subcommand)]
pub enum CtxCommand {
    #[command(about = "Pack files/directories into compact context metadata")]
    Pack(PackArgs),
    #[command(about = "Extract symbols from file(s)")]
    Symbols(SymbolsArgs),
    #[command(about = "Show changed paths from git status")]
    Changed(ChangedArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CtxPreset {
    Summary,
    Review,
    Debug,
}

#[derive(Debug, Args)]
pub struct PackArgs {
    pub paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    pub preset: CtxPreset,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct SymbolsArgs {
    pub path: PathBuf,
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    pub preset: CtxPreset,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct ChangedArgs {}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PresetSettings {
    default_limit: usize,
    pack_symbol_preview_limit: usize,
    symbols_per_file_limit: usize,
}

impl CtxPreset {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Review => "review",
            Self::Debug => "debug",
        }
    }

    pub(crate) fn settings(self) -> PresetSettings {
        match self {
            Self::Summary => PresetSettings {
                default_limit: 80,
                pack_symbol_preview_limit: 4,
                symbols_per_file_limit: 20,
            },
            Self::Review => PresetSettings {
                default_limit: 200,
                pack_symbol_preview_limit: 8,
                symbols_per_file_limit: 80,
            },
            Self::Debug => PresetSettings {
                default_limit: 500,
                pack_symbol_preview_limit: 16,
                symbols_per_file_limit: 200,
            },
        }
    }
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: CtxArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        CtxCommand::Pack(pack_args) => {
            let result = domain::execute_pack(pack_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        CtxCommand::Symbols(symbols_args) => {
            let result = domain::execute_symbols(symbols_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        CtxCommand::Changed(changed_args) => {
            let result = domain::execute_changed(changed_args)?;
            adapters::output::emit(result, options)
        }
    }
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-ctx",
        "ctx",
        vec![
            pack_descriptor(),
            symbols_descriptor(),
            changed_descriptor(),
        ],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = match request.command.as_str() {
        "ctx.pack" => typed_pack(request),
        "ctx.symbols" => typed_symbols(request),
        "ctx.changed" => typed_changed(request),
        _ => {
            return TypedInvocationResponse::error(CommandError::new(
                Some("ctx".to_owned()),
                Some(request.command.clone()),
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown ctx command",
                "the command is not present in the ctx catalog",
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
                .with_domain("ctx")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn typed_pack(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let cwd = Path::new(&request.context.cwd);
    let paths = request
        .arguments
        .get("paths")
        .and_then(Value::as_array)
        .map(|paths| {
            paths
                .iter()
                .filter_map(Value::as_str)
                .map(|path| resolve_context_path(cwd, Path::new(path)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let args = PackArgs {
        paths,
        preset: typed_preset(&request.arguments),
        max_bytes: typed_max_bytes(&request.arguments),
        follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
    };
    let result = domain::execute_pack(args, request.context.limit)?;
    let data = result_to_value(result)?;
    let count = data["item_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Packed {count} context item(s).")))
}

fn typed_symbols(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let cwd = Path::new(&request.context.cwd);
    let path = request
        .arguments
        .get("path")
        .and_then(Value::as_str)
        .expect("validated ctx.symbols input contains path");
    let args = SymbolsArgs {
        path: resolve_context_path(cwd, Path::new(path)),
        preset: typed_preset(&request.arguments),
        max_bytes: typed_max_bytes(&request.arguments),
        follow_symlinks: typed_bool(&request.arguments, "follow_symlinks"),
    };
    let result = domain::execute_symbols(args, request.context.limit)?;
    let data = result_to_value(result)?;
    let count = data["symbol_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Extracted {count} symbol(s).")))
}

fn typed_changed(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let result = domain::execute_changed_at(ChangedArgs {}, Path::new(&request.context.cwd))?;
    let data = result_to_value(result)?;
    let count = data["changed_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Returned {count} changed path(s).")))
}

fn result_to_value(result: domain::CtxResult) -> Result<Value, AppError> {
    let value = match result {
        domain::CtxResult::Pack(value) => serde_json::to_value(value)?,
        domain::CtxResult::Symbols(value) => serde_json::to_value(value)?,
        domain::CtxResult::Changed(value) => serde_json::to_value(value)?,
    };
    Ok(value)
}

fn resolve_context_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn typed_preset(arguments: &Value) -> CtxPreset {
    match arguments
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("review")
    {
        "summary" => CtxPreset::Summary,
        "debug" => CtxPreset::Debug,
        _ => CtxPreset::Review,
    }
}

fn typed_max_bytes(arguments: &Value) -> u64 {
    arguments
        .get("max_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(crate::safety::DEFAULT_MAX_TEXT_BYTES)
}

fn typed_bool(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn pack_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "ctx.pack",
        "Pack context metadata",
        "Create a compact metadata and symbol digest for files and directories.",
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": {"type": "string", "minLength": 1},
                    "description": "Files or directories to pack; defaults to the context cwd."
                },
                "preset": preset_schema(),
                "max_bytes": max_bytes_schema(),
                "follow_symlinks": follow_symlinks_schema()
            },
            "additionalProperties": false
        }),
        pack_output_schema(),
        ctx_read_effects(
            "Reads metadata and eligible text content under the requested paths; following symlinks may read outside those path trees.",
        ),
    )
    .with_example(CommandExample::new(
        "Pack source and documentation for review",
        json!({"paths": ["src", "docs"], "preset": "review"}),
    ))
}

fn symbols_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "ctx.symbols",
        "Extract context symbols",
        "Extract code, configuration, and document symbols from one file or directory.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "File or directory to inspect."
                },
                "preset": preset_schema(),
                "max_bytes": max_bytes_schema(),
                "follow_symlinks": follow_symlinks_schema()
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        symbols_output_schema(),
        ctx_read_effects(
            "Reads eligible text files under the requested path; following symlinks may read outside that path tree.",
        ),
    )
    .with_example(CommandExample::new(
        "Extract summary symbols from commands",
        json!({"path": "src/commands", "preset": "summary"}),
    ))
}

fn changed_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "ctx.changed",
        "List changed context paths",
        "Return changed paths from the Git working tree rooted at the execution cwd.",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        changed_output_schema(),
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![CommandEffect::FilesystemRead, CommandEffect::ProcessSpawn],
            RiskLevel::Low,
            "Runs read-only Git repository checks and reads working-tree status.",
            Reversibility::Yes,
        ),
    )
}

fn ctx_read_effects(impact: &str) -> CommandEffects {
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

fn preset_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["summary", "review", "debug"],
        "default": "review",
        "description": "Controls default limits and symbol density."
    })
}

fn max_bytes_schema() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": crate::safety::DEFAULT_MAX_TEXT_BYTES,
        "description": "Skip text files larger than this byte size."
    })
}

fn follow_symlinks_schema() -> Value {
    json!({
        "type": "boolean",
        "default": false,
        "description": "Follow symlinked files and directories."
    })
}

fn symbol_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "line": {"type": "integer", "minimum": 1},
            "kind": {"type": "string"},
            "name": {"type": "string"}
        },
        "required": ["line", "kind", "name"],
        "additionalProperties": false
    })
}

fn pack_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "ctx.pack"},
            "preset": {"type": "string"},
            "roots": {"type": "array", "items": {"type": "string"}},
            "item_count": {"type": "integer", "minimum": 0},
            "file_count": {"type": "integer", "minimum": 0},
            "directory_count": {"type": "integer", "minimum": 0},
            "symbol_count": {"type": "integer", "minimum": 0},
            "skipped_binary_files": {"type": "integer", "minimum": 0},
            "skipped_large_files": {"type": "integer", "minimum": 0},
            "skipped_symlink_files": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "kind": {"type": "string"},
                        "size_bytes": {"type": "integer", "minimum": 0},
                        "line_count": {"type": "integer", "minimum": 0},
                        "symbol_count": {"type": "integer", "minimum": 0},
                        "symbols": {"type": "array", "items": symbol_schema()}
                    },
                    "required": [
                        "path",
                        "kind",
                        "size_bytes",
                        "line_count",
                        "symbol_count",
                        "symbols"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": [
            "command",
            "preset",
            "roots",
            "item_count",
            "file_count",
            "directory_count",
            "symbol_count",
            "skipped_binary_files",
            "skipped_large_files",
            "skipped_symlink_files",
            "truncated",
            "items"
        ],
        "additionalProperties": false
    })
}

fn symbols_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "ctx.symbols"},
            "preset": {"type": "string"},
            "root": {"type": "string"},
            "file_count": {"type": "integer", "minimum": 0},
            "symbol_count": {"type": "integer", "minimum": 0},
            "skipped_binary_files": {"type": "integer", "minimum": 0},
            "skipped_large_files": {"type": "integer", "minimum": 0},
            "skipped_symlink_files": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "files": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "symbol_count": {"type": "integer", "minimum": 0},
                        "symbols": {"type": "array", "items": symbol_schema()}
                    },
                    "required": ["path", "symbol_count", "symbols"],
                    "additionalProperties": false
                }
            }
        },
        "required": [
            "command",
            "preset",
            "root",
            "file_count",
            "symbol_count",
            "skipped_binary_files",
            "skipped_large_files",
            "skipped_symlink_files",
            "truncated",
            "files"
        ],
        "additionalProperties": false
    })
}

fn changed_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "ctx.changed"},
            "in_git_repo": {"type": "boolean"},
            "changed_count": {"type": "integer", "minimum": 0},
            "entries": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string"},
                        "path": {"type": "string"},
                        "old_path": {"type": ["string", "null"]}
                    },
                    "required": ["status", "path", "old_path"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["command", "in_git_repo", "changed_count", "entries"],
        "additionalProperties": false
    })
}
