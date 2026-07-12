use crate::{cli::GlobalOptions, error::AppError};
use clap::{Args, Subcommand};

const DEFAULT_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub command: TaskCommand,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    #[command(about = "Save a reusable shell command")]
    Save(SaveArgs),
    #[command(about = "Run a saved task by name")]
    Run(RunArgs),
    #[command(about = "List saved tasks")]
    List(ListArgs),
}

#[derive(Debug, Args)]
pub struct SaveArgs {
    pub name: String,
    pub command: String,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    pub name: String,
    #[arg(long, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    pub timeout_secs: u64,
    #[arg(long, default_value_t = DEFAULT_MAX_OUTPUT_BYTES, value_name = "BYTES")]
    pub max_output_bytes: usize,
}

#[derive(Debug, Args)]
pub struct ListArgs {}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: TaskArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let result = domain::execute(args, options.limit)?;
    adapters::output::emit(result, options)
}
