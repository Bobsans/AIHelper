use clap::{Args, Subcommand};

use crate::{cli::GlobalOptions, error::AppError};

#[derive(Debug, Args)]
pub struct GitArgs {
    #[command(subcommand)]
    pub command: GitCommand,
}

#[derive(Debug, Subcommand)]
pub enum GitCommand {
    #[command(about = "Show repository status summary")]
    Status(StatusArgs),
    #[command(about = "List tags newest-first")]
    Tags(TagsArgs),
    #[command(about = "List configured remotes")]
    Remotes(RemotesArgs),
    #[command(about = "Show working tree changes")]
    Changed(ChangedArgs),
    #[command(about = "Show local git diff (optionally filtered by path)")]
    Diff(DiffArgs),
    #[command(about = "Show blame information for a file or a single line")]
    Blame(BlameArgs),
    #[command(about = "Show commit metadata, touched files, and stats")]
    CommitInfo(CommitInfoArgs),
    #[command(about = "Create or inspect git tags")]
    Tag(TagArgs),
}

#[derive(Debug, Args)]
pub struct StatusArgs {}

#[derive(Debug, Args)]
pub struct TagsArgs {
    #[arg(long)]
    pub latest: bool,
}

#[derive(Debug, Args)]
pub struct RemotesArgs {}

#[derive(Debug, Args)]
pub struct ChangedArgs {}

#[derive(Debug, Args)]
pub struct DiffArgs {
    #[arg(long)]
    pub path: Option<std::path::PathBuf>,
}

#[derive(Debug, Args)]
pub struct BlameArgs {
    pub path: std::path::PathBuf,
    #[arg(long)]
    pub line: Option<usize>,
}

#[derive(Debug, Args)]
pub struct CommitInfoArgs {
    #[arg(default_value = "HEAD", value_name = "ref")]
    pub reference: String,
}

#[derive(Debug, Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub command: TagCommand,
}

#[derive(Debug, Subcommand)]
pub enum TagCommand {
    #[command(about = "Create a git tag")]
    Create(TagCreateArgs),
}

#[derive(Debug, Args)]
pub struct TagCreateArgs {
    pub tag: String,
    #[arg(long, value_name = "TEXT")]
    pub message: Option<String>,
    #[arg(long = "ref", default_value = "HEAD", value_name = "ref")]
    pub reference: String,
}

pub(crate) mod io;
pub(crate) mod output;

mod adapters {
    pub(crate) use super::io;
    pub(crate) use super::output;
}

mod domain;

pub fn execute(args: GitArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        GitCommand::Status(status_args) => {
            let result = domain::execute_status(status_args)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Tags(tags_args) => {
            let result = domain::execute_tags(tags_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Remotes(remotes_args) => {
            let result = domain::execute_remotes(remotes_args)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Changed(changed_args) => {
            let result = domain::execute_changed(changed_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Diff(diff_args) => {
            let result = domain::execute_diff(diff_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Blame(blame_args) => {
            let result = domain::execute_blame(blame_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        GitCommand::CommitInfo(commit_args) => {
            let result = domain::execute_commit_info(commit_args, options.limit)?;
            adapters::output::emit(result, options)
        }
        GitCommand::Tag(tag_args) => {
            let result = domain::execute_tag(tag_args)?;
            adapters::output::emit(result, options)
        }
    }
}
