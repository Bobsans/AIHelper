//! `ah git`: one function per subcommand, over `io::GitIo`.
//!
//! The 848 lines this came from also held every output struct and every parser
//! for git's ad-hoc text formats, so the subcommands - the part a reader is
//! usually looking for - were a third of the file scattered through it.
//!
//! | Module   | Owns                                            |
//! |----------|-------------------------------------------------|
//! | `output` | what each subcommand reports                     |
//! | `parse`  | reading git's porcelain, name-status and numstat |

use super::io;
use regex::Regex;
use schemars::JsonSchema;
use serde::Serialize;
use std::{path::Path, sync::OnceLock, thread};

use ah_error::AppError;
use ah_runtime::core::apply_limit;

use crate::git_status::{
    StatusEntry, count_statuses, parse_porcelain_v1_z, parse_porcelain_v2_branch_z,
};

use super::{
    BlameArgs, ChangedArgs, CommitInfoArgs, DiffArgs, RemotesArgs, StatusArgs, TagArgs, TagCommand,
    TagCreateArgs, TagsArgs,
};

mod output;
mod parse;
use output::empty_status_result;
pub(crate) use output::{
    BlameEntry, ChangedEntry, CommitFile, CommitInfo, CommitInfoOutput, CommitSummary,
    GitBlameOutput, GitChangedOutput, GitDiffOutput, GitPerson, GitRemotesOutput, GitResult,
    GitStatusOutput, GitTagCreateOutput, GitTagsOutput, RemoteEntry, TagEntry,
};
use parse::{
    changed_entry, is_no_commit_error, normalize_path, optional_trimmed, parse_line_porcelain,
    parse_name_status, parse_numstat, parse_remotes, short_commit, sum_optional,
};

pub(crate) fn execute(
    args: super::GitArgs,
    limit: Option<usize>,
    cwd: Option<&Path>,
) -> Result<GitResult, AppError> {
    let io = match cwd {
        Some(cwd) => io::GitIo::at(cwd),
        None => io::GitIo::current()?,
    };
    match args.command {
        super::GitCommand::Status(args) => execute_status(args, &io),
        super::GitCommand::Tags(args) => execute_tags(args, limit, &io),
        super::GitCommand::Remotes(args) => execute_remotes(args, &io),
        super::GitCommand::Changed(args) => execute_changed(args, limit, &io),
        super::GitCommand::Diff(args) => execute_diff(args, limit, &io),
        super::GitCommand::Blame(args) => execute_blame(args, limit, &io),
        super::GitCommand::CommitInfo(args) => execute_commit_info(args, limit, &io),
        super::GitCommand::Tag(args) => execute_tag(args, &io),
    }
}

fn execute_status(_args: StatusArgs, io: &io::GitIo) -> Result<GitResult, AppError> {
    let Some(raw_status) = io.read_status_snapshot()? else {
        return Ok(empty_status_result());
    };
    let snapshot = parse_porcelain_v2_branch_z(&raw_status)?;
    let status_entries = snapshot.entries;
    let counts = count_statuses(&status_entries);
    let (latest_commit, latest_tag) = thread::scope(|scope| {
        let commit = scope.spawn(|| {
            io.read_trimmed(["log", "-1", "--format=%H%x00%s"])
                .and_then(|raw| {
                    let (hash, subject) = raw.split_once('\0')?;
                    Some(CommitSummary {
                        hash: hash.to_owned(),
                        short_hash: short_commit(hash),
                        subject: subject.to_owned(),
                    })
                })
        });
        let tag = scope.spawn(|| io.read_trimmed(["describe", "--tags", "--abbrev=0"]));
        (
            commit.join().expect("git log worker should not panic"),
            tag.join().expect("git describe worker should not panic"),
        )
    });

    Ok(GitResult::Status(GitStatusOutput {
        command: "git.status",
        in_git_repo: true,
        branch: snapshot.branch,
        upstream: snapshot.upstream,
        ahead: snapshot.ahead,
        behind: snapshot.behind,
        clean: status_entries.is_empty(),
        staged_count: counts.staged,
        unstaged_count: counts.unstaged,
        untracked_count: counts.untracked,
        changed_count: status_entries.len(),
        latest_commit,
        latest_tag,
    }))
}

fn execute_tags(
    args: TagsArgs,
    limit: Option<usize>,
    io: &io::GitIo,
) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    let mut tags = if in_repo {
        io.read_output(["tag".to_owned(), "--sort=-creatordate".to_owned()])?
            .lines()
            .map(|line| TagEntry {
                name: line.to_owned(),
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if args.latest && tags.len() > 1 {
        tags.truncate(1);
    }
    let truncated = apply_limit(&mut tags, limit);

    Ok(GitResult::Tags(GitTagsOutput {
        command: "git.tags",
        in_git_repo: in_repo,
        latest: args.latest,
        tag_count: tags.len(),
        truncated,
        tags,
    }))
}

fn execute_remotes(_args: RemotesArgs, io: &io::GitIo) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    let remotes = if in_repo {
        parse_remotes(&io.read_output(["remote".to_owned(), "-v".to_owned()])?)
    } else {
        Vec::new()
    };

    Ok(GitResult::Remotes(GitRemotesOutput {
        command: "git.remotes",
        in_git_repo: in_repo,
        remote_count: remotes.len(),
        remotes,
    }))
}

fn execute_changed(
    _args: ChangedArgs,
    limit: Option<usize>,
    io: &io::GitIo,
) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    let mut entries = if in_repo {
        parse_porcelain_v1_z(&io.read_output_bytes([
            "status".to_owned(),
            "--porcelain=v1".to_owned(),
            "-z".to_owned(),
        ])?)?
        .into_iter()
        .map(changed_entry)
        .collect()
    } else {
        Vec::new()
    };

    let truncated = apply_limit(&mut entries, limit);

    Ok(GitResult::Changed(GitChangedOutput {
        command: "git.changed",
        in_git_repo: in_repo,
        changed_count: entries.len(),
        truncated,
        entries,
    }))
}

fn execute_diff(
    args: DiffArgs,
    limit: Option<usize>,
    io: &io::GitIo,
) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    let path_filter = args
        .path
        .as_ref()
        .map(|value| normalize_path(&value.to_string_lossy()));

    let mut diff = if in_repo {
        let mut command = vec!["diff".to_owned(), "--no-color".to_owned()];
        if let Some(path) = args.path {
            command.push("--".to_owned());
            command.push(path.to_string_lossy().into_owned());
        }
        io.read_output(&command)?
    } else {
        String::new()
    };

    let mut diff_lines: Vec<String> = diff.lines().map(|line| line.to_owned()).collect();
    let truncated = apply_limit(&mut diff_lines, limit);
    diff = diff_lines.join("\n");

    Ok(GitResult::Diff(GitDiffOutput {
        command: "git.diff",
        in_git_repo: in_repo,
        path_filter,
        line_count: diff_lines.len(),
        truncated,
        diff,
    }))
}

fn execute_blame(
    args: BlameArgs,
    limit: Option<usize>,
    io: &io::GitIo,
) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    if !in_repo {
        return Ok(GitResult::Blame {
            in_git_repo: false,
            payload: GitBlameOutput {
                command: "git.blame",
                path: normalize_path(&args.path.to_string_lossy()),
                line_filter: args.line,
                entry_count: 0,
                truncated: false,
                entries: Vec::new(),
            },
        });
    }

    if !io.resolve_path(&args.path).exists() {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            args.path.to_string_lossy()
        )));
    }
    if let Some(line) = args.line
        && line == 0
    {
        return Err(AppError::invalid_argument("--line must be >= 1"));
    }

    let path_string = args.path.to_string_lossy().into_owned();
    let porcelain_result = if let Some(line) = args.line {
        io.read_output(vec![
            "blame".to_owned(),
            "--line-porcelain".to_owned(),
            "-L".to_owned(),
            format!("{line},{line}"),
            "--".to_owned(),
            path_string.clone(),
        ])
    } else {
        io.read_output(vec![
            "blame".to_owned(),
            "--line-porcelain".to_owned(),
            "--".to_owned(),
            path_string.clone(),
        ])
    };

    let porcelain = match porcelain_result {
        Ok(raw) => raw,
        Err(error) if is_no_commit_error(&error) => String::new(),
        Err(error) => return Err(error),
    };

    let mut entries = parse_line_porcelain(&porcelain)?;
    let truncated = apply_limit(&mut entries, limit);

    Ok(GitResult::Blame {
        in_git_repo: true,
        payload: GitBlameOutput {
            command: "git.blame",
            path: normalize_path(&path_string),
            line_filter: args.line,
            entry_count: entries.len(),
            truncated,
            entries,
        },
    })
}

fn execute_commit_info(
    args: CommitInfoArgs,
    limit: Option<usize>,
    io: &io::GitIo,
) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    let commit = if in_repo {
        Some(read_commit_info(io, &args.reference, limit)?)
    } else {
        None
    };

    Ok(GitResult::CommitInfo(CommitInfoOutput {
        command: "git.commit-info",
        in_git_repo: in_repo,
        reference: args.reference,
        commit,
    }))
}

fn execute_tag(args: TagArgs, io: &io::GitIo) -> Result<GitResult, AppError> {
    match args.command {
        TagCommand::Create(create_args) => execute_tag_create(create_args, io),
    }
}

fn execute_tag_create(args: TagCreateArgs, io: &io::GitIo) -> Result<GitResult, AppError> {
    let in_repo = io.is_inside_repo()?;
    if !in_repo {
        return Ok(GitResult::TagCreate(GitTagCreateOutput {
            command: "git.tag.create",
            in_git_repo: false,
            tag: args.tag,
            reference: args.reference,
            annotated: args.message.is_some(),
            target_commit: None,
        }));
    }

    let annotated = args.message.is_some();
    let mut command = vec!["tag".to_owned()];
    if let Some(message) = args.message {
        command.push("-a".to_owned());
        command.push(args.tag.clone());
        command.push("-m".to_owned());
        command.push(message);
        command.push(args.reference.clone());
    } else {
        command.push(args.tag.clone());
        command.push(args.reference.clone());
    }
    io.read_output(&command)?;
    let target_ref = format!("{}^{{commit}}", args.tag);
    let target_commit = io
        .read_trimmed(["rev-parse", target_ref.as_str()])
        .map(|hash| CommitSummary {
            short_hash: short_commit(&hash),
            hash,
            subject: io
                .read_trimmed(["log", "-1", "--format=%s", &args.tag])
                .unwrap_or_default(),
        });

    let payload = GitTagCreateOutput {
        command: "git.tag.create",
        in_git_repo: true,
        tag: args.tag,
        reference: args.reference,
        annotated,
        target_commit,
    };
    Ok(GitResult::TagCreate(payload))
}

fn read_commit_info(
    io: &io::GitIo,
    reference: &str,
    limit: Option<usize>,
) -> Result<CommitInfo, AppError> {
    let metadata = io.read_output([
        "show".to_owned(),
        "-s".to_owned(),
        "--format=%H%x00%h%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%s%x00%b".to_owned(),
        reference.to_owned(),
    ])?;
    let mut parts = metadata.splitn(10, '\0');
    let hash = parts.next().unwrap_or("").trim().to_owned();
    let short_hash = parts.next().unwrap_or("").trim().to_owned();
    let author_name = parts.next().unwrap_or("").trim().to_owned();
    let author_email = parts.next().unwrap_or("").trim().to_owned();
    let author_date = optional_trimmed(parts.next().unwrap_or(""));
    let committer_name = parts.next().unwrap_or("").trim().to_owned();
    let committer_email = parts.next().unwrap_or("").trim().to_owned();
    let committer_date = optional_trimmed(parts.next().unwrap_or(""));
    let subject = parts.next().unwrap_or("").trim().to_owned();
    let body = parts.next().unwrap_or("").trim().to_owned();

    let mut files = read_commit_files(io, reference)?;
    let file_count = files.len();
    let additions = sum_optional(files.iter().map(|file| file.additions));
    let deletions = sum_optional(files.iter().map(|file| file.deletions));
    let truncated = apply_limit(&mut files, limit);

    Ok(CommitInfo {
        hash,
        short_hash,
        author: GitPerson {
            name: author_name,
            email: author_email,
        },
        author_date,
        committer: GitPerson {
            name: committer_name,
            email: committer_email,
        },
        committer_date,
        subject,
        body,
        file_count,
        additions,
        deletions,
        files,
        truncated,
    })
}

fn read_commit_files(io: &io::GitIo, reference: &str) -> Result<Vec<CommitFile>, AppError> {
    let status_raw = io.read_output([
        "diff-tree".to_owned(),
        "--no-commit-id".to_owned(),
        "--name-status".to_owned(),
        "-r".to_owned(),
        "--root".to_owned(),
        reference.to_owned(),
    ])?;
    let stats_raw = io.read_output([
        "show".to_owned(),
        "--numstat".to_owned(),
        "--format=".to_owned(),
        "--root".to_owned(),
        reference.to_owned(),
    ])?;
    let stats = parse_numstat(&stats_raw);
    let mut files = parse_name_status(&status_raw);
    for file in &mut files {
        if let Some((additions, deletions)) = stats.iter().find_map(|stat| {
            if stat.path == file.path {
                Some((stat.additions, stat.deletions))
            } else {
                None
            }
        }) {
            file.additions = additions;
            file.deletions = deletions;
        }
    }
    if files.is_empty() {
        files = stats
            .into_iter()
            .map(|stat| CommitFile {
                status: None,
                path: stat.path,
                old_path: None,
                additions: stat.additions,
                deletions: stat.deletions,
            })
            .collect();
    }
    Ok(files)
}
