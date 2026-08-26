use crate::{
    commands::git::domain::{
        CommitInfoOutput, GitBlameOutput, GitChangedOutput, GitDiffOutput, GitRemotesOutput,
        GitResult, GitStatusOutput, GitTagCreateOutput, GitTagsOutput,
    },
    error::AppError,
    output::{Emitter, TextFormatter, TextStyle, git_status_style, render_semantic_count},
};

const NOT_A_REPOSITORY: &str = "not a git repository";
const TRUNCATED: &str = "output truncated by --limit";

pub(crate) fn emit(result: GitResult, emitter: &mut Emitter) -> Result<(), AppError> {
    match result {
        GitResult::Status(payload) => emit_status(payload, emitter),
        GitResult::Tags(payload) => emit_tags(payload, emitter),
        GitResult::Remotes(payload) => emit_remotes(payload, emitter),
        GitResult::Changed(payload) => emit_changed(payload, emitter),
        GitResult::Diff(payload) => emit_diff(payload, emitter),
        GitResult::Blame {
            payload,
            in_git_repo,
        } => emit_blame(payload, in_git_repo, emitter),
        GitResult::CommitInfo(payload) => emit_commit_info(payload, emitter),
        GitResult::TagCreate(payload) => emit_tag_create(payload, emitter),
    }
}

fn not_a_repository(formatter: TextFormatter) -> String {
    formatter.paint(TextStyle::Warning, NOT_A_REPOSITORY)
}

fn emit_status(payload: GitStatusOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if payload.in_git_repo {
            render_status_text(&payload, formatter)
        } else {
            not_a_repository(formatter)
        }
    })
}

fn emit_tags(payload: GitTagsOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        payload
            .tags
            .iter()
            .map(|tag| formatter.paint(TextStyle::Key, &tag.name))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if payload.in_git_repo && payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_remotes(payload: GitRemotesOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        payload
            .remotes
            .iter()
            .map(|remote| {
                format!(
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
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn emit_changed(payload: GitChangedOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        if payload.entries.is_empty() {
            return formatter.paint(TextStyle::Success, "working tree is clean");
        }
        payload
            .entries
            .iter()
            .map(|entry| render_changed_entry(entry, formatter))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if payload.in_git_repo && !payload.entries.is_empty() && payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_diff(payload: GitDiffOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        if payload.diff.is_empty() {
            return formatter.paint(TextStyle::Muted, "no local diff");
        }
        payload.diff.clone()
    })?;
    if payload.in_git_repo && !payload.diff.is_empty() && payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_blame(
    payload: GitBlameOutput,
    in_git_repo: bool,
    emitter: &mut Emitter,
) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !in_git_repo {
            return not_a_repository(formatter);
        }
        if payload.entries.is_empty() {
            return formatter.paint(TextStyle::Muted, "no blame data");
        }
        payload
            .entries
            .iter()
            .map(|entry| {
                format!(
                    "{} {} {} | {}",
                    formatter.paint(TextStyle::Muted, format!("{:>5}", entry.line)),
                    formatter.paint(
                        TextStyle::Key,
                        entry.commit.chars().take(8).collect::<String>()
                    ),
                    formatter.paint(TextStyle::Key, &entry.author),
                    entry.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    if in_git_repo && !payload.entries.is_empty() && payload.truncated {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_commit_info(payload: CommitInfoOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        let Some(commit) = &payload.commit else {
            return formatter.paint(TextStyle::Warning, "commit not found");
        };
        let mut lines = vec![
            format!(
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
            ),
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Muted, format!("files={}", commit.file_count)),
                render_optional_stat("additions", commit.additions, TextStyle::Success, formatter),
                render_optional_stat("deletions", commit.deletions, TextStyle::Error, formatter)
            ),
        ];
        lines.extend(
            commit
                .files
                .iter()
                .map(|file| render_commit_file(file, formatter)),
        );
        lines.join("\n")
    })?;
    if payload.in_git_repo
        && let Some(commit) = &payload.commit
        && commit.truncated
    {
        emitter.text_warning(TRUNCATED);
    }
    Ok(())
}

fn emit_tag_create(payload: GitTagCreateOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    emitter.value(&payload, |formatter| {
        if !payload.in_git_repo {
            return not_a_repository(formatter);
        }
        format!(
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
        )
    })
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
    use super::{emit_tags, render_changed_entry, render_commit_file, render_status_text};
    use crate::{
        cli::GlobalOptions,
        commands::git::domain::{
            ChangedEntry, CommitFile, CommitSummary, GitStatusOutput, GitTagsOutput, TagEntry,
        },
        output::{Emitter, OutputMode, TextFormatter},
    };

    fn options(output: OutputMode, quiet: bool) -> GlobalOptions {
        GlobalOptions {
            output,
            quiet,
            limit: None,
            cwd: None,
        }
    }

    fn tags_output(truncated: bool) -> GitTagsOutput {
        GitTagsOutput {
            command: "git.tags",
            in_git_repo: true,
            latest: false,
            tag_count: 1,
            truncated,
            tags: vec![TagEntry {
                name: "v1.0.0".to_owned(),
            }],
        }
    }

    /// The point of the emitter: output is assertable without a subprocess.
    #[test]
    fn text_mode_writes_the_rendered_lines_and_the_truncation_warning() {
        let (mut emitter, captured) = Emitter::capture(&options(OutputMode::Text, false));

        emit_tags(tags_output(true), &mut emitter).expect("emit should succeed");

        assert_eq!(
            captured.stdout(),
            "v1.0.0
"
        );
        assert_eq!(
            captured.stderr(),
            "warning: output truncated by --limit
"
        );
    }

    #[test]
    fn json_mode_writes_the_payload_and_no_warning() {
        let (mut emitter, captured) = Emitter::capture(&options(OutputMode::Json, false));

        emit_tags(tags_output(true), &mut emitter).expect("emit should succeed");

        let payload: serde_json::Value =
            serde_json::from_str(&captured.stdout()).expect("stdout should be JSON");
        assert_eq!(payload["truncated"], true);
        // The payload already says it was truncated; stderr does not repeat it.
        assert_eq!(captured.stderr(), "");
    }

    #[test]
    fn quiet_suppresses_both_streams() {
        for mode in [OutputMode::Text, OutputMode::Json] {
            let (mut emitter, captured) = Emitter::capture(&options(mode, true));

            emit_tags(tags_output(true), &mut emitter).expect("emit should succeed");

            assert_eq!(captured.stdout(), "", "{mode:?}");
            assert_eq!(captured.stderr(), "", "{mode:?}");
        }
    }

    #[test]
    fn an_empty_list_writes_nothing_rather_than_a_blank_line() {
        let (mut emitter, captured) = Emitter::capture(&options(OutputMode::Text, false));
        let mut payload = tags_output(false);
        payload.tags.clear();
        payload.tag_count = 0;

        emit_tags(payload, &mut emitter).expect("emit should succeed");

        assert_eq!(captured.stdout(), "");
    }

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
