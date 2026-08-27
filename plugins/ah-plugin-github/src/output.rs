//! The text a command prints when the caller did not ask for JSON.
//!
//! Every renderer takes a `TextFormatter`, so the same function produces the
//! plain contract and the styled one; the `*_style` functions below are the
//! only place a state name decides a colour.

use super::*;

pub(crate) fn render_issues_text(issues: &[IssueResponse], formatter: TextFormatter) -> String {
    if issues.is_empty() {
        return String::new();
    }
    issues
        .iter()
        .map(|issue| {
            format!(
                "#{} {} {} {}",
                formatter.paint(TextStyle::Key, issue.number),
                formatter.paint(issue_state_style(&issue.state), &issue.state),
                issue.title,
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    issue.html_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_comments_text(
    comments: &[IssueCommentResponse],
    formatter: TextFormatter,
) -> String {
    if comments.is_empty() {
        return String::new();
    }
    comments
        .iter()
        .map(|comment| {
            let first_line = comment
                .body
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, comment.id),
                formatter.paint(
                    TextStyle::Key,
                    comment
                        .user
                        .as_ref()
                        .map(|user| user.login.as_str())
                        .unwrap_or("-")
                ),
                first_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_release_text(release: &ReleaseResponse, formatter: TextFormatter) -> String {
    format!(
        "{} draft={} prerelease={} assets={} {}\n",
        formatter.paint(TextStyle::Key, &release.tag_name),
        formatter.paint(bool_warning_style(release.draft), release.draft),
        formatter.paint(bool_warning_style(release.prerelease), release.prerelease),
        formatter.paint(TextStyle::Muted, release.assets.len()),
        render::paint_if_present(
            formatter,
            TextStyle::Key,
            release.html_url.as_deref().unwrap_or("")
        )
    )
}

pub(crate) fn render_assets_text(assets: &[ReleaseAsset], formatter: TextFormatter) -> String {
    if assets.is_empty() {
        return String::new();
    }
    assets
        .iter()
        .map(|asset| {
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, &asset.name),
                formatter.paint(TextStyle::Muted, asset.size),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    asset.browser_download_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_workflows_text(
    workflows: &[WorkflowResponse],
    formatter: TextFormatter,
) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    workflows
        .iter()
        .map(|workflow| {
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, workflow.id),
                formatter.paint(workflow_state_style(&workflow.state), &workflow.state),
                formatter.paint(TextStyle::Key, &workflow.path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_runs_text(runs: &[WorkflowRunResponse], formatter: TextFormatter) -> String {
    if runs.is_empty() {
        return String::new();
    }
    runs.iter()
        .map(|run| {
            let conclusion = run.conclusion.as_deref().unwrap_or("-");
            format!(
                "{} {} {} {} {} {}",
                formatter.paint(TextStyle::Key, run.id),
                run.name.as_deref().unwrap_or("-"),
                formatter.paint(TextStyle::Muted, &run.event),
                formatter.paint(execution_status_style(&run.status), &run.status),
                formatter.paint(conclusion_style(conclusion), conclusion),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    run.html_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_jobs_text(jobs: &[JobResponse], formatter: TextFormatter) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    jobs.iter()
        .map(|job| {
            let conclusion = job.conclusion.as_deref().unwrap_or("-");
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, &job.name),
                formatter.paint(execution_status_style(&job.status), &job.status),
                formatter.paint(conclusion_style(conclusion), conclusion)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_artifacts_text(
    artifacts: &[ArtifactResponse],
    formatter: TextFormatter,
) -> String {
    if artifacts.is_empty() {
        return String::new();
    }
    artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} {} expired={}",
                formatter.paint(TextStyle::Key, &artifact.name),
                formatter.paint(TextStyle::Muted, artifact.size_in_bytes),
                formatter.paint(
                    if artifact.expired {
                        TextStyle::Error
                    } else {
                        TextStyle::Success
                    },
                    artifact.expired
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn issue_state_style(state: &str) -> TextStyle {
    match state {
        "open" => TextStyle::Success,
        "closed" => TextStyle::Muted,
        _ => TextStyle::Warning,
    }
}

pub(crate) fn workflow_state_style(state: &str) -> TextStyle {
    match state {
        "active" => TextStyle::Success,
        value if value.starts_with("disabled") => TextStyle::Warning,
        _ => TextStyle::Muted,
    }
}

pub(crate) fn execution_status_style(status: &str) -> TextStyle {
    match status {
        "queued" | "pending" | "in_progress" | "requested" | "waiting" => TextStyle::Warning,
        "success" | "active" => TextStyle::Success,
        "failure" | "failed" | "cancelled" | "timed_out" | "action_required" => TextStyle::Error,
        _ => TextStyle::Muted,
    }
}

pub(crate) fn conclusion_style(conclusion: &str) -> TextStyle {
    match conclusion {
        "success" => TextStyle::Success,
        "failure" | "cancelled" | "timed_out" | "action_required" | "startup_failure" => {
            TextStyle::Error
        }
        "neutral" | "skipped" | "-" => TextStyle::Muted,
        _ => TextStyle::Warning,
    }
}

pub(crate) fn bool_warning_style(value: bool) -> TextStyle {
    if value {
        TextStyle::Warning
    } else {
        TextStyle::Muted
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn issue_renderer_preserves_plain_contract_and_styles_metadata() {
        let issue = IssueResponse {
            number: 42,
            title: "Fix formatter".to_owned(),
            body: Some("raw body".to_owned()),
            state: "open".to_owned(),
            html_url: Some("https://github.com/acme/tool/issues/42".to_owned()),
            user: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: Some(0),
            created_at: None,
            updated_at: None,
            closed_at: None,
            pull_request: None,
        };

        assert_eq!(
            render_issues_text(
                std::slice::from_ref(&issue),
                TextFormatter::with_color(false)
            ),
            "#42 open Fix formatter https://github.com/acme/tool/issues/42\n"
        );

        let rendered = render_issues_text(&[issue], TextFormatter::with_color(true));
        assert!(rendered.contains("#\u{1b}[36m42\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[32mopen\u{1b}[0m"));
        assert!(rendered.contains("Fix formatter"));
        assert!(!rendered.contains("\u{1b}[0mFix formatter"));
    }

    #[test]
    fn workflow_renderers_map_execution_states() {
        let run = WorkflowRunResponse {
            id: 7,
            name: Some("CI".to_owned()),
            event: "push".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("failure".to_owned()),
            head_branch: Some("main".to_owned()),
            head_sha: "abc123".to_owned(),
            html_url: Some("https://github.com/acme/tool/actions/runs/7".to_owned()),
            created_at: None,
            updated_at: None,
        };

        assert_eq!(
            render_runs_text(std::slice::from_ref(&run), TextFormatter::with_color(false)),
            "7 CI push completed failure https://github.com/acme/tool/actions/runs/7\n"
        );

        let rendered = render_runs_text(&[run], TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36m7\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2mcompleted\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;31mfailure\u{1b}[0m"));
    }

    #[test]
    fn artifact_renderer_styles_expiration_without_changing_plain_shape() {
        let artifact = ArtifactResponse {
            id: 8,
            name: "ah-windows.zip".to_owned(),
            size_in_bytes: 123,
            expired: true,
            archive_download_url: None,
        };

        assert_eq!(
            render_artifacts_text(
                std::slice::from_ref(&artifact),
                TextFormatter::with_color(false)
            ),
            "ah-windows.zip 123 expired=true\n"
        );

        let rendered = render_artifacts_text(&[artifact], TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36mah-windows.zip\u{1b}[0m"));
        assert!(rendered.contains("expired=\u{1b}[1;31mtrue\u{1b}[0m"));
    }
}
