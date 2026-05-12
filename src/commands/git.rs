use std::{path::PathBuf, process::Command, sync::OnceLock};

use clap::{Args, Subcommand};
use regex::Regex;
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

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
    pub path: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct BlameArgs {
    pub path: PathBuf,
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

#[derive(Debug, Clone, Serialize)]
struct ChangedEntry {
    status: String,
    path: String,
    old_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct GitChangedOutput {
    command: &'static str,
    in_git_repo: bool,
    changed_count: usize,
    truncated: bool,
    entries: Vec<ChangedEntry>,
}

#[derive(Debug, Serialize)]
struct GitStatusOutput {
    command: &'static str,
    in_git_repo: bool,
    branch: Option<String>,
    upstream: Option<String>,
    ahead: Option<usize>,
    behind: Option<usize>,
    clean: bool,
    staged_count: usize,
    unstaged_count: usize,
    untracked_count: usize,
    changed_count: usize,
    latest_commit: Option<CommitSummary>,
    latest_tag: Option<String>,
}

#[derive(Debug, Serialize)]
struct CommitSummary {
    hash: String,
    short_hash: String,
    subject: String,
}

#[derive(Debug, Serialize)]
struct CommitInfoOutput {
    command: &'static str,
    in_git_repo: bool,
    reference: String,
    commit: Option<CommitInfo>,
}

#[derive(Debug, Serialize)]
struct CommitInfo {
    hash: String,
    short_hash: String,
    author: GitPerson,
    author_date: Option<String>,
    committer: GitPerson,
    committer_date: Option<String>,
    subject: String,
    body: String,
    file_count: usize,
    additions: Option<usize>,
    deletions: Option<usize>,
    files: Vec<CommitFile>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct GitPerson {
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize)]
struct CommitFile {
    status: Option<String>,
    path: String,
    old_path: Option<String>,
    additions: Option<usize>,
    deletions: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
struct TagEntry {
    name: String,
}

#[derive(Debug, Serialize)]
struct GitTagsOutput {
    command: &'static str,
    in_git_repo: bool,
    latest: bool,
    tag_count: usize,
    truncated: bool,
    tags: Vec<TagEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct RemoteEntry {
    name: String,
    fetch_url: Option<String>,
    push_url: Option<String>,
    provider: String,
}

#[derive(Debug, Serialize)]
struct GitRemotesOutput {
    command: &'static str,
    in_git_repo: bool,
    remote_count: usize,
    remotes: Vec<RemoteEntry>,
}

#[derive(Debug, Serialize)]
struct GitDiffOutput {
    command: &'static str,
    in_git_repo: bool,
    path_filter: Option<String>,
    line_count: usize,
    truncated: bool,
    diff: String,
}

#[derive(Debug, Clone, Serialize)]
struct BlameEntry {
    line: usize,
    commit: String,
    author: String,
    author_mail: String,
    author_time: Option<i64>,
    summary: String,
    text: String,
}

#[derive(Debug, Serialize)]
struct GitBlameOutput {
    command: &'static str,
    path: String,
    line_filter: Option<usize>,
    entry_count: usize,
    truncated: bool,
    entries: Vec<BlameEntry>,
}

#[derive(Debug, Serialize)]
struct GitTagCreateOutput {
    command: &'static str,
    in_git_repo: bool,
    tag: String,
    reference: String,
    annotated: bool,
    target_commit: Option<CommitSummary>,
}

pub fn execute(args: GitArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        GitCommand::Status(status_args) => execute_status(status_args, options),
        GitCommand::Tags(tags_args) => execute_tags(tags_args, options),
        GitCommand::Remotes(remotes_args) => execute_remotes(remotes_args, options),
        GitCommand::Changed(changed_args) => execute_changed(changed_args, options),
        GitCommand::Diff(diff_args) => execute_diff(diff_args, options),
        GitCommand::Blame(blame_args) => execute_blame(blame_args, options),
        GitCommand::CommitInfo(commit_args) => execute_commit_info(commit_args, options),
        GitCommand::Tag(tag_args) => execute_tag(tag_args, options),
    }
}

fn execute_status(_args: StatusArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let raw_status = if in_repo {
        read_git_output(vec!["status".to_owned(), "--porcelain".to_owned()])?
    } else {
        String::new()
    };
    let entries = parse_porcelain_status(raw_status.clone());
    let (staged_count, unstaged_count, untracked_count) = count_porcelain_status(&raw_status);
    let branch = if in_repo {
        read_git_trimmed(["branch", "--show-current"])
    } else {
        None
    };
    let upstream = if in_repo {
        read_git_trimmed(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
    } else {
        None
    };
    let (ahead, behind) = if in_repo && upstream.is_some() {
        read_git_trimmed(["rev-list", "--left-right", "--count", "@{u}...HEAD"])
            .and_then(|raw| parse_ahead_behind(&raw))
            .unwrap_or((None, None))
    } else {
        (None, None)
    };
    let latest_commit = if in_repo {
        read_git_trimmed(["log", "-1", "--format=%H%x00%s"]).and_then(|raw| {
            let (hash, subject) = raw.split_once('\0')?;
            Some(CommitSummary {
                hash: hash.to_owned(),
                short_hash: short_commit(hash),
                subject: subject.to_owned(),
            })
        })
    } else {
        None
    };
    let latest_tag = if in_repo {
        read_git_trimmed(["describe", "--tags", "--abbrev=0"])
    } else {
        None
    };

    if options.quiet {
        return Ok(());
    }

    let payload = GitStatusOutput {
        command: "git.status",
        in_git_repo: in_repo,
        branch,
        upstream,
        ahead,
        behind,
        clean: entries.is_empty(),
        staged_count,
        unstaged_count,
        untracked_count,
        changed_count: entries.len(),
        latest_commit,
        latest_tag,
    };

    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            println!(
                "branch={} upstream={} ahead={} behind={} clean={}",
                payload.branch.as_deref().unwrap_or("-"),
                payload.upstream.as_deref().unwrap_or("-"),
                payload
                    .ahead
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                payload
                    .behind
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                payload.clean
            );
            println!(
                "changed={} staged={} unstaged={} untracked={}",
                payload.changed_count,
                payload.staged_count,
                payload.unstaged_count,
                payload.untracked_count
            );
            if let Some(commit) = &payload.latest_commit {
                println!("commit={} {}", commit.short_hash, commit.subject);
            }
            if let Some(tag) = &payload.latest_tag {
                println!("latest_tag={tag}");
            }
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

fn execute_tags(args: TagsArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let mut tags = if in_repo {
        read_git_output(vec!["tag".to_owned(), "--sort=-creatordate".to_owned()])?
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
    let truncated = apply_limit(&mut tags, options.limit);

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            for tag in &tags {
                println!("{}", tag.name);
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = GitTagsOutput {
                command: "git.tags",
                in_git_repo: in_repo,
                latest: args.latest,
                tag_count: tags.len(),
                truncated,
                tags,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_remotes(_args: RemotesArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let remotes = if in_repo {
        parse_remotes(&read_git_output(vec![
            "remote".to_owned(),
            "-v".to_owned(),
        ])?)
    } else {
        Vec::new()
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            for remote in &remotes {
                println!(
                    "{} fetch={} push={} provider={}",
                    remote.name,
                    remote.fetch_url.as_deref().unwrap_or("-"),
                    remote.push_url.as_deref().unwrap_or("-"),
                    remote.provider
                );
            }
        }
        OutputMode::Json => {
            let payload = GitRemotesOutput {
                command: "git.remotes",
                in_git_repo: in_repo,
                remote_count: remotes.len(),
                remotes,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_changed(_args: ChangedArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let mut entries = if in_repo {
        parse_porcelain_status(read_git_output(vec![
            "status".to_owned(),
            "--porcelain".to_owned(),
        ])?)
    } else {
        Vec::new()
    };

    let truncated = apply_limit(&mut entries, options.limit);

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            if entries.is_empty() {
                println!("working tree is clean");
                return Ok(());
            }
            for entry in &entries {
                match &entry.old_path {
                    Some(old_path) => println!("{} {} -> {}", entry.status, old_path, entry.path),
                    None => println!("{} {}", entry.status, entry.path),
                }
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = GitChangedOutput {
                command: "git.changed",
                in_git_repo: in_repo,
                changed_count: entries.len(),
                truncated,
                entries,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_diff(args: DiffArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let path_filter = args
        .path
        .as_ref()
        .map(|value| normalize_path(value.as_path()));

    let mut diff = if in_repo {
        let mut command = vec!["diff".to_owned(), "--no-color".to_owned()];
        if let Some(path) = args.path {
            command.push("--".to_owned());
            command.push(path.to_string_lossy().into_owned());
        }
        read_git_output(command)?
    } else {
        String::new()
    };

    let mut diff_lines: Vec<String> = diff.lines().map(|line| line.to_owned()).collect();
    let truncated = apply_limit(&mut diff_lines, options.limit);
    diff = diff_lines.join("\n");

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            if diff.is_empty() {
                println!("no local diff");
                return Ok(());
            }
            println!("{diff}");
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = GitDiffOutput {
                command: "git.diff",
                in_git_repo: in_repo,
                path_filter,
                line_count: diff_lines.len(),
                truncated,
                diff,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_blame(args: BlameArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    if !in_repo {
        if options.quiet {
            return Ok(());
        }
        return match options.output {
            OutputMode::Text => {
                println!("not a git repository");
                Ok(())
            }
            OutputMode::Json => {
                let payload = GitBlameOutput {
                    command: "git.blame",
                    path: normalize_path(args.path.as_path()),
                    line_filter: args.line,
                    entry_count: 0,
                    truncated: false,
                    entries: Vec::new(),
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
                Ok(())
            }
        };
    }

    if !args.path.exists() {
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
        read_git_output(vec![
            "blame".to_owned(),
            "--line-porcelain".to_owned(),
            "-L".to_owned(),
            format!("{line},{line}"),
            "--".to_owned(),
            path_string.clone(),
        ])
    } else {
        read_git_output(vec![
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
    let truncated = apply_limit(&mut entries, options.limit);

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if entries.is_empty() {
                println!("no blame data");
                return Ok(());
            }
            for entry in &entries {
                println!(
                    "{:>5} {} {} | {}",
                    entry.line,
                    short_commit(&entry.commit),
                    entry.author,
                    entry.text
                );
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = GitBlameOutput {
                command: "git.blame",
                path: normalize_path(args.path.as_path()),
                line_filter: args.line,
                entry_count: entries.len(),
                truncated,
                entries,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_commit_info(args: CommitInfoArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;

    let commit = if in_repo {
        Some(read_commit_info(&args.reference, options.limit)?)
    } else {
        None
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            let Some(commit) = &commit else {
                println!("commit not found");
                return Ok(());
            };
            println!(
                "commit={} author=\"{} <{}>\" date={} subject={}",
                commit.short_hash,
                commit.author.name,
                commit.author.email,
                commit.author_date.as_deref().unwrap_or("-"),
                commit.subject
            );
            println!(
                "files={} additions={} deletions={}",
                commit.file_count,
                commit
                    .additions
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                commit
                    .deletions
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "-".to_owned())
            );
            for file in &commit.files {
                println!(
                    "{} +{} -{} {}",
                    file.status.as_deref().unwrap_or("-"),
                    file.additions
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    file.deletions
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_owned()),
                    file.path
                );
            }
            if commit.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = CommitInfoOutput {
                command: "git.commit-info",
                in_git_repo: in_repo,
                reference: args.reference,
                commit,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_tag(args: TagArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        TagCommand::Create(create_args) => execute_tag_create(create_args, options),
    }
}

fn execute_tag_create(args: TagCreateArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    if !in_repo {
        if options.quiet {
            return Ok(());
        }
        return match options.output {
            OutputMode::Text => {
                println!("not a git repository");
                Ok(())
            }
            OutputMode::Json => {
                let payload = GitTagCreateOutput {
                    command: "git.tag.create",
                    in_git_repo: false,
                    tag: args.tag,
                    reference: args.reference,
                    annotated: args.message.is_some(),
                    target_commit: None,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
                Ok(())
            }
        };
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
    read_git_output(command)?;
    let target_commit =
        read_git_trimmed(["rev-parse", &format!("{}^{{commit}}", args.tag)]).map(|hash| {
            CommitSummary {
                short_hash: short_commit(&hash),
                hash,
                subject: read_git_trimmed(["log", "-1", "--format=%s", &args.tag])
                    .unwrap_or_default(),
            }
        });

    if options.quiet {
        return Ok(());
    }

    let payload = GitTagCreateOutput {
        command: "git.tag.create",
        in_git_repo: true,
        tag: args.tag,
        reference: args.reference,
        annotated,
        target_commit,
    };

    match options.output {
        OutputMode::Text => {
            println!(
                "created tag {} at {}",
                payload.tag,
                payload
                    .target_commit
                    .as_ref()
                    .map(|commit| commit.short_hash.as_str())
                    .unwrap_or("-")
            );
        }
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(&payload)?),
    }

    Ok(())
}

fn read_git_output(args: Vec<String>) -> Result<String, AppError> {
    let printable = format!("git {}", args.join(" "));
    let output = Command::new("git")
        .args(args.iter().map(String::as_str))
        .output()
        .map_err(|source| AppError::command_execution(printable.clone(), source))?;
    if !output.status.success() {
        return Err(AppError::command_failed(
            printable,
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn read_commit_info(reference: &str, limit: Option<usize>) -> Result<CommitInfo, AppError> {
    let metadata = read_git_output(vec![
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

    let mut files = read_commit_files(reference)?;
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

fn read_commit_files(reference: &str) -> Result<Vec<CommitFile>, AppError> {
    let status_raw = read_git_output(vec![
        "diff-tree".to_owned(),
        "--no-commit-id".to_owned(),
        "--name-status".to_owned(),
        "-r".to_owned(),
        "--root".to_owned(),
        reference.to_owned(),
    ])?;
    let stats_raw = read_git_output(vec![
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

fn read_git_trimmed<const N: usize>(args: [&str; N]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() { None } else { Some(value) }
}

fn is_inside_git_repo() -> Result<bool, AppError> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|source| {
            AppError::command_execution("git rev-parse --is-inside-work-tree", source)
        })?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn parse_porcelain_status(raw: String) -> Vec<ChangedEntry> {
    let mut entries = Vec::new();
    for line in raw.lines() {
        let mut chars = line.chars();
        let x = chars.next().unwrap_or(' ');
        let y = chars.next().unwrap_or(' ');
        let _ = chars.next();
        let rest: String = chars.collect();
        if rest.is_empty() {
            continue;
        }
        let status = format!("{x}{y}").trim().to_owned();
        if let Some((old_path, new_path)) = rest.split_once(" -> ") {
            entries.push(ChangedEntry {
                status,
                path: normalize_slashes(new_path),
                old_path: Some(normalize_slashes(old_path)),
            });
        } else {
            entries.push(ChangedEntry {
                status,
                path: normalize_slashes(&rest),
                old_path: None,
            });
        }
    }
    entries
}

fn count_porcelain_status(raw: &str) -> (usize, usize, usize) {
    let mut staged_count = 0usize;
    let mut unstaged_count = 0usize;
    let mut untracked_count = 0usize;

    for line in raw.lines() {
        if line.len() < 2 {
            continue;
        }
        let mut status = line.chars().take(2);
        let index_status = status.next().unwrap_or(' ');
        let worktree_status = status.next().unwrap_or(' ');

        if index_status == '?' && worktree_status == '?' {
            untracked_count += 1;
            continue;
        }
        if index_status != ' ' {
            staged_count += 1;
        }
        if worktree_status != ' ' {
            unstaged_count += 1;
        }
    }

    (staged_count, unstaged_count, untracked_count)
}

fn blame_header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([0-9a-f^]{7,40})\s+\d+\s+(\d+)(?:\s+\d+)?$").unwrap())
}

fn parse_line_porcelain(raw: &str) -> Result<Vec<BlameEntry>, AppError> {
    let header_re = blame_header_regex();

    let mut entries = Vec::new();
    let mut lines = raw.lines().peekable();

    while let Some(line) = lines.next() {
        let Some(captures) = header_re.captures(line) else {
            continue;
        };

        let commit = captures[1].to_owned();
        let final_line = captures[2].parse::<usize>().unwrap_or(0);

        let mut author = String::new();
        let mut author_mail = String::new();
        let mut author_time = None;
        let mut summary = String::new();
        let mut text = String::new();

        for metadata_line in lines.by_ref() {
            if let Some(value) = metadata_line.strip_prefix('\t') {
                text = value.to_owned();
                break;
            }
            if let Some(value) = metadata_line.strip_prefix("author ") {
                author = value.to_owned();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("author-mail ") {
                author_mail = value.trim_matches(['<', '>']).to_owned();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("author-time ") {
                author_time = value.parse::<i64>().ok();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("summary ") {
                summary = value.to_owned();
            }
        }

        entries.push(BlameEntry {
            line: final_line,
            commit,
            author,
            author_mail,
            author_time,
            summary,
            text,
        });
    }

    Ok(entries)
}

fn parse_name_status(raw: &str) -> Vec<CommitFile> {
    raw.lines()
        .filter_map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            let status = parts.first()?.to_string();
            if status.starts_with('R') || status.starts_with('C') {
                let old_path = parts.get(1).map(|value| normalize_slashes(value));
                let path = parts.get(2).map(|value| normalize_slashes(value))?;
                Some(CommitFile {
                    status: Some(status),
                    path,
                    old_path,
                    additions: None,
                    deletions: None,
                })
            } else {
                let path = parts.get(1).map(|value| normalize_slashes(value))?;
                Some(CommitFile {
                    status: Some(status),
                    path,
                    old_path: None,
                    additions: None,
                    deletions: None,
                })
            }
        })
        .collect()
}

fn parse_numstat(raw: &str) -> Vec<CommitFile> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let additions = parse_optional_usize(parts.next()?);
            let deletions = parse_optional_usize(parts.next()?);
            let path = normalize_slashes(parts.next()?);
            Some(CommitFile {
                status: None,
                path,
                old_path: None,
                additions,
                deletions,
            })
        })
        .collect()
}

fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit
        && items.len() > limit_value
    {
        items.truncate(limit_value);
        return true;
    }
    false
}

fn parse_ahead_behind(raw: &str) -> Option<(Option<usize>, Option<usize>)> {
    let mut parts = raw.split_whitespace();
    let behind = parts.next()?.parse::<usize>().ok()?;
    let ahead = parts.next()?.parse::<usize>().ok()?;
    Some((Some(ahead), Some(behind)))
}

fn parse_optional_usize(raw: &str) -> Option<usize> {
    raw.parse::<usize>().ok()
}

fn optional_trimmed(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

fn sum_optional(values: impl Iterator<Item = Option<usize>>) -> Option<usize> {
    let mut saw_value = false;
    let mut total = 0usize;
    for value in values.flatten() {
        saw_value = true;
        total += value;
    }
    if saw_value { Some(total) } else { None }
}

fn parse_remotes(raw: &str) -> Vec<RemoteEntry> {
    let mut remotes: Vec<RemoteEntry> = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        let Some(kind) = parts.next() else { continue };
        let entry_index = remotes
            .iter()
            .position(|entry| entry.name == name)
            .unwrap_or_else(|| {
                remotes.push(RemoteEntry {
                    name: name.to_owned(),
                    fetch_url: None,
                    push_url: None,
                    provider: "unknown".to_owned(),
                });
                remotes.len() - 1
            });
        let entry = &mut remotes[entry_index];
        match kind {
            "(fetch)" => entry.fetch_url = Some(url.to_owned()),
            "(push)" => entry.push_url = Some(url.to_owned()),
            _ => {}
        }
        entry.provider = detect_provider(entry.fetch_url.as_deref().or(entry.push_url.as_deref()));
    }
    remotes
}

fn detect_provider(url: Option<&str>) -> String {
    let Some(url) = url else {
        return "unknown".to_owned();
    };
    let lower = url.to_ascii_lowercase();
    if lower.contains("github.com") {
        "github".to_owned()
    } else if lower.contains("gitlab.com") {
        "gitlab".to_owned()
    } else if lower.contains("bitbucket.org") {
        "bitbucket".to_owned()
    } else {
        "unknown".to_owned()
    }
}

fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalize_path(path: &std::path::Path) -> String {
    normalize_slashes(&path.to_string_lossy())
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
}

fn is_no_commit_error(error: &AppError) -> bool {
    match error {
        AppError::CommandFailed { stderr, .. } => {
            stderr.contains("no such ref: HEAD")
                || stderr.contains("has no commits yet")
                || stderr.contains("no commits yet")
        }
        _ => false,
    }
}
