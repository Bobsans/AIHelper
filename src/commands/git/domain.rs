use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

use crate::error::AppError;
use ah_runtime::core::apply_limit;

use super::{
    adapters, BlameArgs, ChangedArgs, CommitInfoArgs, DiffArgs, RemotesArgs, StatusArgs, TagArgs, TagCreateArgs,
    TagCommand, TagsArgs,
};

#[derive(Debug, Serialize)]
pub(crate) struct ChangedEntry {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitChangedOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub changed_count: usize,
    pub truncated: bool,
    pub entries: Vec<ChangedEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitStatusOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub clean: bool,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    pub changed_count: usize,
    pub latest_commit: Option<CommitSummary>,
    pub latest_tag: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommitSummary {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) subject: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommitInfoOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub reference: String,
    pub commit: Option<CommitInfo>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommitInfo {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) author: GitPerson,
    pub(crate) author_date: Option<String>,
    pub(crate) committer: GitPerson,
    pub(crate) committer_date: Option<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) file_count: usize,
    pub(crate) additions: Option<usize>,
    pub(crate) deletions: Option<usize>,
    pub(crate) files: Vec<CommitFile>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitPerson {
    pub(crate) name: String,
    pub(crate) email: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommitFile {
    pub(crate) status: Option<String>,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) additions: Option<usize>,
    pub(crate) deletions: Option<usize>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TagEntry {
    pub(crate) name: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitTagsOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub latest: bool,
    pub tag_count: usize,
    pub truncated: bool,
    pub tags: Vec<TagEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RemoteEntry {
    pub(crate) name: String,
    pub(crate) fetch_url: Option<String>,
    pub(crate) push_url: Option<String>,
    pub(crate) provider: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitRemotesOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub remote_count: usize,
    pub remotes: Vec<RemoteEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitDiffOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub path_filter: Option<String>,
    pub line_count: usize,
    pub truncated: bool,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BlameEntry {
    pub(crate) line: usize,
    pub(crate) commit: String,
    pub(crate) author: String,
    pub(crate) author_mail: String,
    pub(crate) author_time: Option<i64>,
    pub(crate) summary: String,
    pub(crate) text: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitBlameOutput {
    pub command: &'static str,
    pub path: String,
    pub line_filter: Option<usize>,
    pub entry_count: usize,
    pub truncated: bool,
    pub entries: Vec<BlameEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GitTagCreateOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub tag: String,
    pub reference: String,
    pub annotated: bool,
    pub target_commit: Option<CommitSummary>,
}

#[derive(Debug)]
pub(crate) enum GitResult {
    Status(GitStatusOutput),
    Tags(GitTagsOutput),
    Remotes(GitRemotesOutput),
    Changed(GitChangedOutput),
    Diff(GitDiffOutput),
    Blame {
        payload: GitBlameOutput,
        in_git_repo: bool,
    },
    CommitInfo(CommitInfoOutput),
    TagCreate(GitTagCreateOutput),
}

pub(crate) fn execute_status(_args: StatusArgs) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let raw_status = if in_repo {
        adapters::io::read_git_output(["status".to_owned(), "--porcelain".to_owned()])?
    } else {
        String::new()
    };
    let entries = parse_porcelain_status(&raw_status);
    let (staged_count, unstaged_count, untracked_count) = count_porcelain_status(&raw_status);
    let branch = if in_repo {
        adapters::io::read_git_trimmed(["branch", "--show-current"])
    } else {
        None
    };
    let upstream = if in_repo {
        adapters::io::read_git_trimmed([
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{u}",
        ])
    } else {
        None
    };
    let (ahead, behind) = if in_repo && upstream.is_some() {
        adapters::io::read_git_trimmed(["rev-list", "--left-right", "--count", "@{u}...HEAD"])
            .and_then(|raw| parse_ahead_behind(&raw))
            .unwrap_or((None, None))
    } else {
        (None, None)
    };
    let latest_commit = if in_repo {
        adapters::io::read_git_trimmed(["log", "-1", "--format=%H%x00%s"])
            .and_then(|raw| {
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
        adapters::io::read_git_trimmed(["describe", "--tags", "--abbrev=0"])
    } else {
        None
    };

    Ok(GitResult::Status(GitStatusOutput {
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
    }))
}

pub(crate) fn execute_tags(args: TagsArgs, limit: Option<usize>) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let mut tags = if in_repo {
        adapters::io::read_git_output(["tag".to_owned(), "--sort=-creatordate".to_owned()])?
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

pub(crate) fn execute_remotes(_args: RemotesArgs) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let remotes = if in_repo {
        parse_remotes(&adapters::io::read_git_output([
            "remote".to_owned(),
            "-v".to_owned(),
        ])?)
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

pub(crate) fn execute_changed(_args: ChangedArgs, limit: Option<usize>) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let mut entries = if in_repo {
        parse_porcelain_status(&adapters::io::read_git_output([
            "status".to_owned(),
            "--porcelain".to_owned(),
        ])?)
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

pub(crate) fn execute_diff(args: DiffArgs, limit: Option<usize>) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let path_filter = args.path.as_ref().map(|value| normalize_path(&value.to_string_lossy()));

    let mut diff = if in_repo {
        let mut command = vec!["diff".to_owned(), "--no-color".to_owned()];
        if let Some(path) = args.path {
            command.push("--".to_owned());
            command.push(path.to_string_lossy().into_owned());
        }
        adapters::io::read_git_output(&command)?
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

pub(crate) fn execute_blame(args: BlameArgs, limit: Option<usize>) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
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

    if !args.path.exists() {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            args.path.to_string_lossy()
        )));
    }
    if let Some(line) = args.line && line == 0 {
        return Err(AppError::invalid_argument("--line must be >= 1"));
    }

    let path_string = args.path.to_string_lossy().into_owned();
    let porcelain_result = if let Some(line) = args.line {
        adapters::io::read_git_output(vec![
            "blame".to_owned(),
            "--line-porcelain".to_owned(),
            "-L".to_owned(),
            format!("{line},{line}"),
            "--".to_owned(),
            path_string.clone(),
        ])
    } else {
        adapters::io::read_git_output(vec![
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

pub(crate) fn execute_commit_info(
    args: CommitInfoArgs,
    limit: Option<usize>,
) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let commit = if in_repo {
        Some(read_commit_info(&args.reference, limit)?)
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

pub(crate) fn execute_tag(args: TagArgs) -> Result<GitResult, AppError> {
    match args.command {
        TagCommand::Create(create_args) => execute_tag_create(create_args),
    }
}

fn execute_tag_create(args: TagCreateArgs) -> Result<GitResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
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
    adapters::io::read_git_output(&command)?;
    let target_ref = format!("{}^{{commit}}", args.tag);
    let target_commit = adapters::io::read_git_trimmed([
        "rev-parse",
        target_ref.as_str(),
    ])
    .and_then(|hash| {
        Some(CommitSummary {
            short_hash: short_commit(&hash),
            hash,
            subject: adapters::io::read_git_trimmed(["log", "-1", "--format=%s", &args.tag])
                .unwrap_or_default(),
        })
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

fn read_commit_info(reference: &str, limit: Option<usize>) -> Result<CommitInfo, AppError> {
    let metadata = adapters::io::read_git_output([
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
    let status_raw = adapters::io::read_git_output([
        "diff-tree".to_owned(),
        "--no-commit-id".to_owned(),
        "--name-status".to_owned(),
        "-r".to_owned(),
        "--root".to_owned(),
        reference.to_owned(),
    ])?;
    let stats_raw = adapters::io::read_git_output([
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

fn parse_porcelain_status(raw: &str) -> Vec<ChangedEntry> {
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

fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
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

fn sum_optional(values: impl Iterator<Item = Option<usize>>) -> Option<usize> {
    let mut saw_value = false;
    let mut total = 0usize;
    for value in values.flatten() {
        saw_value = true;
        total += value;
    }
    if saw_value { Some(total) } else { None }
}

fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalize_path(path: &str) -> String {
    normalize_slashes(path)
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
