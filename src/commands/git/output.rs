use crate::{
    cli::GlobalOptions,
    commands::git::domain::{
        CommitInfoOutput, GitBlameOutput, GitChangedOutput, GitDiffOutput, GitRemotesOutput,
        GitResult, GitStatusOutput, GitTagCreateOutput, GitTagsOutput,
    },
    error::AppError,
    output::{
        OutputMode, TextFormatter, TextStyle, emit_warning, git_status_style, render_semantic_count,
    },
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            println!("{}", render_status_text(&payload, TextFormatter::stdout()));
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            let formatter = TextFormatter::stdout();
            for tag in &payload.tags {
                println!("{}", formatter.paint(TextStyle::Key, &tag.name));
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            let formatter = TextFormatter::stdout();
            for remote in &payload.remotes {
                println!(
                    "{} {} {} {}",
                    formatter.paint(TextStyle::Key, &remote.name),
                    formatter.paint(
                        TextStyle::Muted,
                        format!("fetch={}", remote.fetch_url.as_deref().unwrap_or("-"))
                    ),
                    formatter.paint(
                        TextStyle::Muted,
                        format!("push={}", remote.push_url.as_deref().unwrap_or("-"))
                    ),
                    formatter.paint(TextStyle::Key, format!("provider={}", remote.provider))
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            if payload.entries.is_empty() {
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Success, "working tree is clean")
                );
                return Ok(());
            }
            let formatter = TextFormatter::stdout();
            for entry in &payload.entries {
                println!("{}", render_changed_entry(entry, formatter));
            }
            if payload.truncated {
                emit_warning("output truncated by --limit");
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            if payload.diff.is_empty() {
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Muted, "no local diff")
                );
                return Ok(());
            }
            println!("{}", payload.diff);
            if payload.truncated {
                emit_warning("output truncated by --limit");
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            if !payload.entries.is_empty() {
                let formatter = TextFormatter::stdout();
                for entry in &payload.entries {
                    println!(
                        "{} {} {} | {}",
                        formatter.paint(TextStyle::Muted, format!("{:>5}", entry.line)),
                        formatter.paint(
                            TextStyle::Key,
                            entry.commit.chars().take(8).collect::<String>()
                        ),
                        formatter.paint(TextStyle::Key, &entry.author),
                        entry.text
                    );
                }
                if payload.truncated {
                    emit_warning("output truncated by --limit");
                }
            }
            if payload.entries.is_empty() {
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Muted, "no blame data")
                );
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
                return Ok(());
            }
            let Some(commit) = &payload.commit else {
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "commit not found")
                );
                return Ok(());
            };
            let formatter = TextFormatter::stdout();
            println!(
                "{} {} {} {}{}",
                formatter.paint(TextStyle::Key, format!("commit={}", commit.short_hash)),
                formatter.paint(
                    TextStyle::Muted,
                    format!(
                        "author=\"{} <{}>\"",
                        commit.author.name, commit.author.email
                    )
                ),
                formatter.paint(
                    TextStyle::Muted,
                    format!("date={}", commit.author_date.as_deref().unwrap_or("-"))
                ),
                formatter.paint(TextStyle::Muted, "subject="),
                commit.subject
            );
            println!(
                "{} {} {}",
                formatter.paint(TextStyle::Muted, format!("files={}", commit.file_count)),
                render_optional_stat("additions", commit.additions, TextStyle::Success, formatter),
                render_optional_stat("deletions", commit.deletions, TextStyle::Error, formatter)
            );
            for file in &commit.files {
                println!("{}", render_commit_file(file, formatter));
            }
            if commit.truncated {
                emit_warning("output truncated by --limit");
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
                println!(
                    "{}",
                    TextFormatter::stdout().paint(TextStyle::Warning, "not a git repository")
                );
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
            let formatter = TextFormatter::stdout();
            println!(
                "{} {} {} {}",
                formatter.paint(TextStyle::Success, "created tag"),
                formatter.paint(TextStyle::Key, &payload.tag),
                formatter.paint(TextStyle::Success, "at"),
                formatter.paint(
                    TextStyle::Key,
                    payload
                        .target_commit
                        .as_ref()
                        .map(|commit| commit.short_hash.as_str())
                        .unwrap_or("-")
                )
            );
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn render_status_text(payload: &GitStatusOutput, formatter: TextFormatter) -> String {
    let clean_style = if payload.clean {
        TextStyle::Success
    } else {
        TextStyle::Warning
    };
    let mut lines = vec![
        format!(
            "{} {} {} {} {}",
            formatter.paint(
                TextStyle::Key,
                format!("branch={}", payload.branch.as_deref().unwrap_or("-"))
            ),
            formatter.paint(
                TextStyle::Key,
                format!("upstream={}", payload.upstream.as_deref().unwrap_or("-"))
            ),
            render_optional_count("ahead", payload.ahead, TextStyle::Warning, formatter),
            render_optional_count("behind", payload.behind, TextStyle::Warning, formatter),
            formatter.paint(clean_style, format!("clean={}", payload.clean))
        ),
        format!(
            "{} {} {} {}",
            render_semantic_count(
                "changed",
                payload.changed_count,
                TextStyle::Warning,
                formatter
            ),
            render_semantic_count(
                "staged",
                payload.staged_count,
                TextStyle::Success,
                formatter
            ),
            render_semantic_count(
                "unstaged",
                payload.unstaged_count,
                TextStyle::Warning,
                formatter
            ),
            render_semantic_count(
                "untracked",
                payload.untracked_count,
                TextStyle::Warning,
                formatter
            )
        ),
    ];
    if let Some(commit) = &payload.latest_commit {
        lines.push(format!(
            "{} {}",
            formatter.paint(TextStyle::Key, format!("commit={}", commit.short_hash)),
            commit.subject
        ));
    }
    if let Some(tag) = &payload.latest_tag {
        lines.push(formatter.paint(TextStyle::Key, format!("latest_tag={tag}")));
    }
    lines.join("\n")
}

fn render_changed_entry(
    entry: &crate::commands::git::domain::ChangedEntry,
    formatter: TextFormatter,
) -> String {
    let status = formatter.paint(git_status_style(&entry.status), &entry.status);
    match &entry.old_path {
        Some(old_path) => format!(
            "{} {} -> {}",
            status,
            formatter.paint(TextStyle::Key, old_path),
            formatter.paint(TextStyle::Key, &entry.path)
        ),
        None => format!(
            "{} {}",
            status,
            formatter.paint(TextStyle::Key, &entry.path)
        ),
    }
}

fn render_commit_file(
    file: &crate::commands::git::domain::CommitFile,
    formatter: TextFormatter,
) -> String {
    let status = file.status.as_deref().unwrap_or("-");
    format!(
        "{} {} {} {}",
        formatter.paint(git_status_style(status), status),
        render_prefixed_stat("+", file.additions, TextStyle::Success, formatter),
        render_prefixed_stat("-", file.deletions, TextStyle::Error, formatter),
        formatter.paint(TextStyle::Key, &file.path)
    )
}

fn render_optional_count(
    label: &str,
    value: Option<usize>,
    non_zero_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    match value {
        Some(value) => render_semantic_count(label, value, non_zero_style, formatter),
        None => formatter.paint(TextStyle::Muted, format!("{label}=-")),
    }
}

fn render_optional_stat(
    label: &str,
    value: Option<usize>,
    non_zero_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    match value {
        Some(value) => render_semantic_count(label, value, non_zero_style, formatter),
        None => formatter.paint(TextStyle::Muted, format!("{label}=-")),
    }
}

fn render_prefixed_stat(
    prefix: &str,
    value: Option<usize>,
    non_zero_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    match value {
        Some(value) => {
            let style = if value == 0 {
                TextStyle::Muted
            } else {
                non_zero_style
            };
            formatter.paint(style, format!("{prefix}{value}"))
        }
        None => formatter.paint(TextStyle::Muted, format!("{prefix}-")),
    }
}

#[cfg(test)]
mod tests {
    use super::{render_changed_entry, render_commit_file, render_status_text};
    use crate::{
        commands::git::domain::{ChangedEntry, CommitFile, CommitSummary, GitStatusOutput},
        output::TextFormatter,
    };

    #[test]
    fn status_renderer_preserves_plain_contract() {
        let payload = status_output();

        assert_eq!(
            render_status_text(&payload, TextFormatter::with_color(false)),
            "branch=main upstream=origin/main ahead=2 behind=0 clean=false\n\
             changed=2 staged=1 unstaged=1 untracked=0\n\
             commit=abc1234 initial\n\
             latest_tag=v1.0.0"
        );
    }

    #[test]
    fn changed_renderer_styles_status_and_paths() {
        let entry = ChangedEntry {
            status: "R ".to_owned(),
            path: "new.rs".to_owned(),
            old_path: Some("old.rs".to_owned()),
        };

        assert_eq!(
            render_changed_entry(&entry, TextFormatter::with_color(false)),
            "R  old.rs -> new.rs"
        );
        let rendered = render_changed_entry(&entry, TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36mR \u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mold.rs\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mnew.rs\u{1b}[0m"));
    }

    #[test]
    fn commit_file_renderer_preserves_plain_contract() {
        let file = CommitFile {
            status: Some("M".to_owned()),
            path: "src/lib.rs".to_owned(),
            old_path: None,
            additions: Some(3),
            deletions: Some(1),
        };

        assert_eq!(
            render_commit_file(&file, TextFormatter::with_color(false)),
            "M +3 -1 src/lib.rs"
        );
    }

    fn status_output() -> GitStatusOutput {
        GitStatusOutput {
            command: "git.status",
            in_git_repo: true,
            branch: Some("main".to_owned()),
            upstream: Some("origin/main".to_owned()),
            ahead: Some(2),
            behind: Some(0),
            clean: false,
            staged_count: 1,
            unstaged_count: 1,
            untracked_count: 0,
            changed_count: 2,
            latest_commit: Some(CommitSummary {
                hash: "abc123456789".to_owned(),
                short_hash: "abc1234".to_owned(),
                subject: "initial".to_owned(),
            }),
            latest_tag: Some("v1.0.0".to_owned()),
        }
    }
}
