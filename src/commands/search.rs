use crate::{cli::GlobalOptions, error::AppError};
use clap::{Args, Subcommand};

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
}

#[derive(Debug, Args)]
pub struct FilesArgs {
    pub query: String,
    #[arg(value_name = "PATH")]
    pub paths: Vec<std::path::PathBuf>,
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

pub fn execute(args: SearchArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = match args.command {
        SearchCommand::Text(text_args) => domain::execute_text(text_args, options.limit)?,
        SearchCommand::Files(files_args) => domain::execute_files(files_args, options.limit)?,
    };
    adapters::output::emit(result, options)
}
