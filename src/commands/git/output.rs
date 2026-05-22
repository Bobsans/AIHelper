use crate::{
    cli::GlobalOptions,
    commands::git::domain::{
        CommitInfoOutput, GitBlameOutput, GitChangedOutput, GitDiffOutput, GitRemotesOutput, GitResult,
        GitStatusOutput, GitTagCreateOutput, GitTagsOutput,
    },
    error::AppError,
    output::OutputMode,
};

pub(crate) fn emit(result: GitResult, options: &GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    match result {
        GitResult::Status(payload) => emit_status(payload, options),
        GitResult::Tags(payload) => emit_tags(payload, options),
        GitResult::Remotes(payload) => emit_remotes(payload, options),
        GitResult::Changed(payload) => emit_changed(payload, options),
        GitResult::Diff(payload) => emit_diff(payload, options),
        GitResult::Blame {
            payload,
            in_git_repo,
        } => emit_blame(payload, in_git_repo, options),
        GitResult::CommitInfo(payload) => emit_commit_info(payload, options),
        GitResult::TagCreate(payload) => emit_tag_create(payload, options),
    }
}

fn emit_status(payload: GitStatusOutput, options: &GlobalOptions) -> Result<(), AppError> {
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
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_tags(payload: GitTagsOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            for tag in &payload.tags {
                println!("{}", tag.name);
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_remotes(payload: GitRemotesOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            for remote in &payload.remotes {
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
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_changed(payload: GitChangedOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            if payload.entries.is_empty() {
                println!("working tree is clean");
                return Ok(());
            }
            for entry in &payload.entries {
                match &entry.old_path {
                    Some(old_path) => println!("{} {} -> {}", entry.status, old_path, entry.path),
                    None => println!("{} {}", entry.status, entry.path),
                }
            }
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_diff(payload: GitDiffOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            if payload.diff.is_empty() {
                println!("no local diff");
                return Ok(());
            }
            println!("{}", payload.diff);
            if payload.truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_blame(
    payload: GitBlameOutput,
    in_git_repo: bool,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            if !payload.entries.is_empty() {
                for entry in &payload.entries {
                    println!(
                        "{:>5} {} {} | {}",
                        entry.line,
                        entry.commit.chars().take(8).collect::<String>(),
                        entry.author,
                        entry.text
                    );
                }
                if payload.truncated {
                    eprintln!("warning: output truncated by --limit");
                }
            }
            if payload.entries.is_empty() {
                println!("no blame data");
            }
            Ok(())
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
            Ok(())
        }
    }
}

fn emit_commit_info(payload: CommitInfoOutput, options: &GlobalOptions) -> Result<(), AppError> {
    match options.output {
        OutputMode::Text => {
            if !payload.in_git_repo {
                println!("not a git repository");
                return Ok(());
            }
            let Some(commit) = &payload.commit else {
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
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn emit_tag_create(payload: GitTagCreateOutput, options: &GlobalOptions) -> Result<(), AppError> {
    if !payload.in_git_repo {
        match options.output {
            OutputMode::Text => {
                println!("not a git repository");
                Ok::<(), AppError>(())
            }
            OutputMode::Json => {
                println!("{}", serde_json::to_string_pretty(&payload)?);
                Ok::<(), AppError>(())
            }
        }?;
        return Ok(());
    }

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
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}
