use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{commands, error::AppError, output::OutputMode};

#[derive(Debug, Parser)]
#[command(
    name = "ah",
    version,
    about = "AIHelper CLI toolbox for AI agents and developers"
)]
pub struct Cli {
    #[arg(long, global = true, help = "Return machine-readable JSON output")]
    pub json: bool,
    #[arg(long, global = true, help = "Suppress command output")]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Set working directory"
    )]
    pub cwd: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        value_name = "N",
        help = "Cap output lines when supported"
    )]
    pub limit: Option<usize>,
    #[command(subcommand)]
    pub domain: Domain,
}

#[derive(Debug, Subcommand)]
pub enum Domain {
    File(commands::file::FileArgs),
    Search(commands::search::SearchArgs),
    Ctx(commands::ctx::CtxArgs),
    Git(commands::git::GitArgs),
    Task(commands::task::TaskArgs),
}

#[derive(Debug, Clone, Copy)]
pub struct GlobalOptions {
    pub output: OutputMode,
    pub quiet: bool,
    pub limit: Option<usize>,
}

impl Cli {
    pub fn execute(self) -> Result<(), AppError> {
        if let Some(cwd) = self.cwd.clone() {
            std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
        }
        if self.limit == Some(0) {
            return Err(AppError::invalid_argument("--limit must be >= 1"));
        }

        let options = GlobalOptions {
            output: if self.json {
                OutputMode::Json
            } else {
                OutputMode::Text
            },
            quiet: self.quiet,
            limit: self.limit,
        };

        match self.domain {
            Domain::File(args) => commands::file::execute(args, &options),
            Domain::Search(args) => commands::search::execute(args, &options),
            Domain::Ctx(args) => commands::ctx::execute(args, &options),
            Domain::Git(args) => commands::git::execute(args, &options),
            Domain::Task(args) => commands::task::execute(args, &options),
        }
    }
}
