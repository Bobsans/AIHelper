use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions};

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

#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CtxPreset {
    Summary,
    #[default]
    Review,
    Debug,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackArgs {
    /// Files or directories to pack; defaults to the context cwd.
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    pub paths: Vec<PathBuf>,
    /// Controls default limits and symbol density.
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    #[serde(default)]
    #[schemars(default)]
    pub preset: CtxPreset,
    /// Skip text files larger than this byte size.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Follow symlinked files and directories.
    #[arg(long, help = "Follow symlink directories during traversal")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
}

fn default_max_bytes() -> u64 {
    crate::safety::DEFAULT_MAX_TEXT_BYTES
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SymbolsArgs {
    /// File or directory to inspect.
    #[schemars(extend("minLength" = 1))]
    pub path: PathBuf,
    /// Controls default limits and symbol density.
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    #[serde(default)]
    #[schemars(default)]
    pub preset: CtxPreset,
    /// Skip text files larger than this byte size.
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = crate::safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    #[serde(default = "default_max_bytes")]
    #[schemars(default = "default_max_bytes", range(min = 1))]
    pub max_bytes: u64,
    /// Follow symlinked files and directories.
    #[arg(long, help = "Follow symlink directories during traversal")]
    #[serde(default)]
    #[schemars(default)]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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

pub mod io;
pub(crate) mod output;

mod domain;
pub mod symbols;

pub fn execute(args: CtxArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let cwd = options.cwd.as_deref();
    match args.command {
        CtxCommand::Pack(mut pack_args) => {
            if let Some(cwd) = cwd {
                rebase_pack(&mut pack_args, cwd);
            }
            let result = domain::execute_pack(pack_args, options.limit)?;
            output::emit(result, &mut Emitter::stdio(options))
        }
        CtxCommand::Symbols(mut symbols_args) => {
            if let Some(cwd) = cwd {
                symbols_args.path = resolve_context_path(cwd, &symbols_args.path);
            }
            let result = domain::execute_symbols(symbols_args, options.limit)?;
            output::emit(result, &mut Emitter::stdio(options))
        }
        CtxCommand::Changed(changed_args) => {
            let result = match cwd {
                Some(cwd) => domain::execute_changed_at(changed_args, cwd)?,
                None => domain::execute_changed(changed_args)?,
            };
            output::emit(result, &mut Emitter::stdio(options))
        }
    }
}

/// Resolve every packed path against the directory the request named.
///
/// Shared by both entry points: the CLI used to get this by the process having
/// been `chdir`-ed, which is the same answer only as long as one request is in
/// flight at a time.
fn rebase_pack(args: &mut PackArgs, cwd: &Path) {
    args.paths = args
        .paths
        .iter()
        .map(|path| resolve_context_path(cwd, path))
        .collect();
}

pub fn command_catalog() -> CommandCatalog {
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

pub fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
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
    let mut args: PackArgs = decode(request)?;
    rebase_pack(&mut args, Path::new(&request.context.cwd));
    let result = domain::execute_pack(args, request.context.limit)?;
    let data = result_to_value(result)?;
    let count = data["item_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Packed {count} context item(s).")))
}

fn typed_symbols(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let cwd = Path::new(&request.context.cwd);
    let mut args: SymbolsArgs = decode(request)?;
    args.path = resolve_context_path(cwd, &args.path);
    let result = domain::execute_symbols(args, request.context.limit)?;
    let data = result_to_value(result)?;
    let count = data["symbol_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Extracted {count} symbol(s).")))
}

fn typed_changed(request: &TypedInvocationRequest) -> Result<(Value, String), AppError> {
    let args: ChangedArgs = decode(request)?;
    let result = domain::execute_changed_at(args, Path::new(&request.context.cwd))?;
    let data = result_to_value(result)?;
    let count = data["changed_count"].as_u64().unwrap_or(0);
    Ok((data, format!("Returned {count} changed path(s).")))
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

fn pack_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "ctx.pack",
        "Pack context metadata",
        "Create a compact metadata and symbol digest for files and directories.",
        input_schema_for::<PackArgs>(),
        output_schema_for::<domain::CtxPackOutput>("ctx.pack"),
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
        input_schema_for::<SymbolsArgs>(),
        output_schema_for::<domain::CtxSymbolsOutput>("ctx.symbols"),
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
        input_schema_for::<ChangedArgs>(),
        output_schema_for::<domain::CtxChangedOutput>("ctx.changed"),
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
