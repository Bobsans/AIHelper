use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

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
