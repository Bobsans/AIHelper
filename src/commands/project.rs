use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::{cli::GlobalOptions, error::AppError};

mod rules;

mod adapters {
    pub mod io;
    pub mod output;
}

mod domain;

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    #[command(about = "Detect project ecosystems and important files")]
    Detect(ProjectPathArgs),
    #[command(about = "Suggest common project commands")]
    Commands(ProjectPathArgs),
    #[command(about = "Detect project version from common manifest files")]
    Version(ProjectPathArgs),
}

#[derive(Debug, Args)]
pub struct ProjectPathArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

pub fn execute(args: ProjectArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        ProjectCommand::Detect(path_args) => execute_detect(path_args, options),
        ProjectCommand::Commands(path_args) => execute_commands(path_args, options),
        ProjectCommand::Version(path_args) => execute_version(path_args, options),
    }
}

fn execute_detect(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_detect(args)?;
    adapters::output::emit_detect(output, options)
}

fn execute_commands(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_commands(args)?;
    adapters::output::emit_commands(output, options)
}

fn execute_version(args: ProjectPathArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let output = domain::run_version(args, options.limit)?;
    adapters::output::emit_version(output, options)
}
