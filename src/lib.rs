pub mod cli;
pub mod commands;
pub mod error;
pub mod output;

use clap::Parser;

use crate::{cli::Cli, error::AppError};

pub fn run() -> Result<(), AppError> {
    let cli = Cli::parse();
    cli.execute()
}
