use std::path::{Path, PathBuf};

use crate::{cli::GlobalOptions, error::AppError};
use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

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
    let result = domain::execute(args, options.limit, None)?;
    adapters::output::emit(result, options)
}

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-git",
        "git",
        vec![
            status_descriptor(),
            tags_descriptor(),
            tag_create_descriptor(),
            remotes_descriptor(),
            changed_descriptor(),
            diff_descriptor(),
            blame_descriptor(),
            commit_info_descriptor(),
        ],
    )
}

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = typed_args(request)
        .and_then(|args| {
            domain::execute(
                args,
                request.context.limit,
                Some(Path::new(&request.context.cwd)),
            )
        })
        .and_then(result_to_value);
    match result {
        Ok(data) => TypedInvocationResponse::success(data, Some(git_result_text(&request.command))),
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("git")
                .with_operation(request.command.clone()),
            false,
        )),
    }
}

fn typed_args(request: &TypedInvocationRequest) -> Result<GitArgs, AppError> {
    let arguments = &request.arguments;
    let command = match request.command.as_str() {
        "git.status" => GitCommand::Status(StatusArgs {}),
        "git.tags" => GitCommand::Tags(TagsArgs {
            latest: typed_bool(arguments, "latest"),
        }),
        "git.tag.create" => GitCommand::Tag(TagArgs {
            command: TagCommand::Create(TagCreateArgs {
                tag: required_string(arguments, "tag")?,
                message: optional_string(arguments, "message"),
                reference: optional_string(arguments, "reference")
                    .unwrap_or_else(|| "HEAD".to_owned()),
            }),
        }),
        "git.remotes" => GitCommand::Remotes(RemotesArgs {}),
        "git.changed" => GitCommand::Changed(ChangedArgs {}),
        "git.diff" => GitCommand::Diff(DiffArgs {
            path: optional_string(arguments, "path").map(PathBuf::from),
        }),
        "git.blame" => GitCommand::Blame(BlameArgs {
            path: PathBuf::from(required_string(arguments, "path")?),
            line: typed_usize(arguments, "line"),
        }),
        "git.commit-info" => GitCommand::CommitInfo(CommitInfoArgs {
            reference: optional_string(arguments, "reference").unwrap_or_else(|| "HEAD".to_owned()),
        }),
        _ => {
            return Err(AppError::invalid_argument(format!(
                "unknown typed git command: {}",
                request.command
            )));
        }
    };
    Ok(GitArgs { command })
}

fn result_to_value(result: domain::GitResult) -> Result<Value, AppError> {
    Ok(match result {
        domain::GitResult::Status(value) => serde_json::to_value(value)?,
        domain::GitResult::Tags(value) => serde_json::to_value(value)?,
        domain::GitResult::Remotes(value) => serde_json::to_value(value)?,
        domain::GitResult::Changed(value) => serde_json::to_value(value)?,
        domain::GitResult::Diff(value) => serde_json::to_value(value)?,
        domain::GitResult::Blame { payload, .. } => serde_json::to_value(payload)?,
        domain::GitResult::CommitInfo(value) => serde_json::to_value(value)?,
        domain::GitResult::TagCreate(value) => serde_json::to_value(value)?,
    })
}

fn git_result_text(command: &str) -> String {
    format!("Completed {command}.")
}

fn typed_bool(arguments: &Value, name: &str) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn typed_usize(arguments: &Value, name: &str) -> Option<usize> {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn required_string(arguments: &Value, name: &str) -> Result<String, AppError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AppError::invalid_argument(format!("missing {name}")))
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn status_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.status",
        "Git repository status",
        "Return branch, upstream, working-tree counts, latest commit, and latest tag.",
        empty_input_schema(),
        status_output_schema(),
        git_read_effects("Runs read-only Git commands and reads repository metadata and status."),
    )
}

fn tags_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.tags",
        "List Git tags",
        "List repository tags newest-first with optional latest-only and output limits.",
        json!({
            "type": "object",
            "properties": {
                "latest": {
                    "type": "boolean",
                    "default": false,
                    "description": "Return at most the newest tag."
                }
            },
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "git.tags"},
                "in_git_repo": {"type": "boolean"},
                "latest": {"type": "boolean"},
                "tag_count": {"type": "integer", "minimum": 0},
                "truncated": {"type": "boolean"},
                "tags": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {"name": {"type": "string"}},
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            },
            "required": [
                "command",
                "in_git_repo",
                "latest",
                "tag_count",
                "truncated",
                "tags"
            ],
            "additionalProperties": false
        }),
        git_read_effects("Runs read-only Git tag enumeration in the repository."),
    )
}

fn tag_create_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.tag.create",
        "Create Git tag",
        "Create a lightweight or annotated local Git tag at a reference.",
        json!({
            "type": "object",
            "properties": {
                "tag": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Tag name to create."
                },
                "message": {
                    "type": "string",
                    "description": "Annotation message; omission creates a lightweight tag."
                },
                "reference": {
                    "type": "string",
                    "minLength": 1,
                    "default": "HEAD",
                    "description": "Commit-ish to tag."
                }
            },
            "required": ["tag"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "git.tag.create"},
                "in_git_repo": {"type": "boolean"},
                "tag": {"type": "string"},
                "reference": {"type": "string"},
                "annotated": {"type": "boolean"},
                "target_commit": nullable_schema(commit_summary_schema())
            },
            "required": [
                "command",
                "in_git_repo",
                "tag",
                "reference",
                "annotated",
                "target_commit"
            ],
            "additionalProperties": false
        }),
        CommandEffects::new(
            false,
            false,
            false,
            false,
            vec![CommandEffect::FilesystemWrite, CommandEffect::ProcessSpawn],
            RiskLevel::Medium,
            "Creates a persistent local Git reference; a conflicting tag fails and reversal requires deleting the tag.",
            Reversibility::Yes,
        ),
    )
    .with_example(CommandExample::new(
        "Create an annotated release tag",
        json!({"tag": "v1.0.0", "message": "v1.0.0"}),
    ))
}

fn remotes_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.remotes",
        "List Git remotes",
        "Return configured fetch and push URLs with provider hints.",
        empty_input_schema(),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "git.remotes"},
                "in_git_repo": {"type": "boolean"},
                "remote_count": {"type": "integer", "minimum": 0},
                "remotes": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "fetch_url": {"type": ["string", "null"]},
                            "push_url": {"type": ["string", "null"]},
                            "provider": {"type": "string"}
                        },
                        "required": ["name", "fetch_url", "push_url", "provider"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["command", "in_git_repo", "remote_count", "remotes"],
            "additionalProperties": false
        }),
        git_read_effects("Runs read-only Git configuration inspection and may reveal remote URLs."),
    )
}

fn changed_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.changed",
        "List Git working-tree changes",
        "Return bounded staged, unstaged, untracked, and renamed paths.",
        empty_input_schema(),
        changed_output_schema(),
        git_read_effects("Runs read-only Git status and returns changed repository paths."),
    )
}

fn diff_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.diff",
        "Read local Git diff",
        "Return the unstaged local Git diff, optionally restricted to one path.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Optional repository-relative path filter."
                }
            },
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "git.diff"},
                "in_git_repo": {"type": "boolean"},
                "path_filter": {"type": ["string", "null"]},
                "line_count": {"type": "integer", "minimum": 0},
                "truncated": {"type": "boolean"},
                "diff": {"type": "string"}
            },
            "required": [
                "command",
                "in_git_repo",
                "path_filter",
                "line_count",
                "truncated",
                "diff"
            ],
            "additionalProperties": false
        }),
        git_read_effects("Runs read-only Git diff and may expose uncommitted source or secrets."),
    )
}

fn blame_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.blame",
        "Read Git blame",
        "Return blame metadata for a repository file or one selected line.",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Repository-relative file path."
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional one-based source line."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "const": "git.blame"},
                "path": {"type": "string"},
                "line_filter": {"type": ["integer", "null"], "minimum": 1},
                "entry_count": {"type": "integer", "minimum": 0},
                "truncated": {"type": "boolean"},
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "line": {"type": "integer", "minimum": 1},
                            "commit": {"type": "string"},
                            "author": {"type": "string"},
                            "author_mail": {"type": "string"},
                            "author_time": {"type": ["integer", "null"]},
                            "summary": {"type": "string"},
                            "text": {"type": "string"}
                        },
                        "required": [
                            "line",
                            "commit",
                            "author",
                            "author_mail",
                            "author_time",
                            "summary",
                            "text"
                        ],
                        "additionalProperties": false
                    }
                }
            },
            "required": [
                "command",
                "path",
                "line_filter",
                "entry_count",
                "truncated",
                "entries"
            ],
            "additionalProperties": false
        }),
        git_read_effects(
            "Runs read-only Git blame and exposes commit authorship metadata and source text.",
        ),
    )
}

fn commit_info_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "git.commit-info",
        "Read Git commit information",
        "Return commit metadata, message, changed files, and line statistics.",
        json!({
            "type": "object",
            "properties": {
                "reference": {
                    "type": "string",
                    "minLength": 1,
                    "default": "HEAD",
                    "description": "Commit-ish to inspect."
                }
            },
            "additionalProperties": false
        }),
        commit_info_output_schema(),
        git_read_effects(
            "Runs read-only Git history queries and exposes commit authors, messages, and paths.",
        ),
    )
    .with_example(CommandExample::new(
        "Inspect the latest commit",
        json!({"reference": "HEAD"}),
    ))
}

fn empty_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false
    })
}

fn git_read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        false,
        vec![CommandEffect::FilesystemRead, CommandEffect::ProcessSpawn],
        RiskLevel::Low,
        impact,
        Reversibility::Yes,
    )
}

fn nullable_schema(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn commit_summary_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "hash": {"type": "string"},
            "short_hash": {"type": "string"},
            "subject": {"type": "string"}
        },
        "required": ["hash", "short_hash", "subject"],
        "additionalProperties": false
    })
}

fn changed_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "git.changed"},
            "in_git_repo": {"type": "boolean"},
            "changed_count": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
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
        "required": [
            "command",
            "in_git_repo",
            "changed_count",
            "truncated",
            "entries"
        ],
        "additionalProperties": false
    })
}

fn status_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "git.status"},
            "in_git_repo": {"type": "boolean"},
            "branch": {"type": ["string", "null"]},
            "upstream": {"type": ["string", "null"]},
            "ahead": {"type": ["integer", "null"], "minimum": 0},
            "behind": {"type": ["integer", "null"], "minimum": 0},
            "clean": {"type": "boolean"},
            "staged_count": {"type": "integer", "minimum": 0},
            "unstaged_count": {"type": "integer", "minimum": 0},
            "untracked_count": {"type": "integer", "minimum": 0},
            "changed_count": {"type": "integer", "minimum": 0},
            "latest_commit": nullable_schema(commit_summary_schema()),
            "latest_tag": {"type": ["string", "null"]}
        },
        "required": [
            "command",
            "in_git_repo",
            "branch",
            "upstream",
            "ahead",
            "behind",
            "clean",
            "staged_count",
            "unstaged_count",
            "untracked_count",
            "changed_count",
            "latest_commit",
            "latest_tag"
        ],
        "additionalProperties": false
    })
}

fn commit_info_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "git.commit-info"},
            "in_git_repo": {"type": "boolean"},
            "reference": {"type": "string"},
            "commit": nullable_schema(json!({
                "type": "object",
                "properties": {
                    "hash": {"type": "string"},
                    "short_hash": {"type": "string"},
                    "author": person_schema(),
                    "author_date": {"type": ["string", "null"]},
                    "committer": person_schema(),
                    "committer_date": {"type": ["string", "null"]},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                    "file_count": {"type": "integer", "minimum": 0},
                    "additions": {"type": ["integer", "null"], "minimum": 0},
                    "deletions": {"type": ["integer", "null"], "minimum": 0},
                    "files": {
                        "type": "array",
                        "items": commit_file_schema()
                    },
                    "truncated": {"type": "boolean"}
                },
                "required": [
                    "hash",
                    "short_hash",
                    "author",
                    "author_date",
                    "committer",
                    "committer_date",
                    "subject",
                    "body",
                    "file_count",
                    "additions",
                    "deletions",
                    "files",
                    "truncated"
                ],
                "additionalProperties": false
            }))
        },
        "required": ["command", "in_git_repo", "reference", "commit"],
        "additionalProperties": false
    })
}

fn person_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "email": {"type": "string"}
        },
        "required": ["name", "email"],
        "additionalProperties": false
    })
}

fn commit_file_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "status": {"type": ["string", "null"]},
            "path": {"type": "string"},
            "old_path": {"type": ["string", "null"]},
            "additions": {"type": ["integer", "null"], "minimum": 0},
            "deletions": {"type": ["integer", "null"], "minimum": 0}
        },
        "required": ["status", "path", "old_path", "additions", "deletions"],
        "additionalProperties": false
    })
}
