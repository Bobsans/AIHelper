use crate::{cli::GlobalOptions, error::AppError};
use clap::{Args, Subcommand};

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
