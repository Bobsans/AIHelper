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
                formatter.paint(TextStyle::Key, issue.iid),
                formatter.paint(issue_state_style(&issue.state), &issue.state),
                issue.title,
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    issue.web_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_comments_text(
    comments: &[IssueNoteResponse],
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
                        .author
                        .as_ref()
                        .and_then(|user| user.username.as_deref())
                        .unwrap_or("-")
                ),
                first_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_issue_full_text(
    issue: &IssueResponse,
    comments: &[IssueNoteResponse],
    designs: &[IssueDesignResponse],
    warnings: &[String],
    formatter: TextFormatter,
) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "#{} {} {}\n",
        formatter.paint(TextStyle::Key, issue.iid),
        formatter.paint(issue_state_style(&issue.state), &issue.state),
        issue.title
    ));
    if let Some(web_url) = &issue.web_url {
        output.push_str(&format!(
            "{} {}\n",
            formatter.paint(TextStyle::Muted, "url:"),
            formatter.paint(TextStyle::Key, web_url)
        ));
    }
    if let Some(author) = &issue.author {
        output.push_str(&format!(
            "{} {}\n",
            formatter.paint(TextStyle::Muted, "author:"),
            formatter.paint(TextStyle::Key, render_user(author))
        ));
    }
    let assignees = issue
        .assignees
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(render_user)
        .collect::<Vec<_>>()
        .join(", ");
    let assignees = if assignees.is_empty() {
        "-".to_owned()
    } else {
        assignees
    };
    output.push_str(&format!(
        "{} {}\n",
        formatter.paint(TextStyle::Muted, "assignees:"),
        formatter.paint(optional_value_style(&assignees), &assignees)
    ));
    let labels = if issue.labels.is_empty() {
        "-".to_owned()
    } else {
        issue.labels.join(", ")
    };
    output.push_str(&format!(
        "{} {}\n",
        formatter.paint(TextStyle::Muted, "labels:"),
        formatter.paint(optional_value_style(&labels), &labels)
    ));
    output.push_str(&format!(
        "{} {}\n{} {}\n{} {}\n",
        formatter.paint(TextStyle::Muted, "created:"),
        formatter.paint(TextStyle::Muted, issue.created_at.as_deref().unwrap_or("-")),
        formatter.paint(TextStyle::Muted, "updated:"),
        formatter.paint(TextStyle::Muted, issue.updated_at.as_deref().unwrap_or("-")),
        formatter.paint(TextStyle::Muted, "closed:"),
        formatter.paint(TextStyle::Muted, issue.closed_at.as_deref().unwrap_or("-"))
    ));
    if let Some(description) = issue
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        output.push_str(&format!(
            "\n{}\n",
            formatter.paint(TextStyle::Heading, "description:")
        ));
        output.push_str(description);
        if !description.ends_with('\n') {
            output.push('\n');
        }
    }
    output.push_str(&format!(
        "\n{}\n",
        formatter.paint(
            TextStyle::Heading,
            format!("comments ({}):", comments.len())
        )
    ));
    if comments.is_empty() {
        output.push_str(&format!(
            "{}\n",
            formatter.paint(TextStyle::Muted, "(none)")
        ));
    } else {
        output.push_str(&render_full_comments_text(comments, formatter));
    }
    output.push_str(&format!(
        "\n{}\n",
        formatter.paint(TextStyle::Heading, format!("designs ({}):", designs.len()))
    ));
    if designs.is_empty() {
        output.push_str(&format!(
            "{}\n",
            formatter.paint(TextStyle::Muted, "(none)")
        ));
    } else {
        output.push_str(&render_designs_text(designs, formatter));
    }
    if !warnings.is_empty() {
        output.push_str(&format!(
            "\n{}\n",
            formatter.paint(TextStyle::Warning, "warnings:")
        ));
        for warning in warnings {
            output.push_str(&format!(
                "- {}\n",
                formatter.paint(TextStyle::Warning, warning)
            ));
        }
    }
    output
}

pub(crate) fn render_full_comments_text(
    comments: &[IssueNoteResponse],
    formatter: TextFormatter,
) -> String {
    comments
        .iter()
        .map(|comment| {
            let body = comment.body.as_deref().unwrap_or("");
            let mut rendered = format!(
                "- {} {} {}\n",
                formatter.paint(TextStyle::Key, comment.id),
                formatter.paint(
                    TextStyle::Key,
                    comment
                        .author
                        .as_ref()
                        .map(render_user)
                        .unwrap_or_else(|| "-".to_owned())
                ),
                formatter.paint(
                    TextStyle::Muted,
                    comment.created_at.as_deref().unwrap_or("-")
                )
            );
            if !body.is_empty() {
                rendered.push_str(body);
                if !body.ends_with('\n') {
                    rendered.push('\n');
                }
            }
            rendered
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_designs_text(
    designs: &[IssueDesignResponse],
    formatter: TextFormatter,
) -> String {
    designs
        .iter()
        .map(|design| {
            let event = design.event.as_deref().unwrap_or("-");
            format!(
                "{} {} {} {}",
                formatter.paint(TextStyle::Key, design.filename.as_deref().unwrap_or("-")),
                formatter.paint(execution_status_style(event), event),
                formatter.paint(TextStyle::Muted, design.notes_count.unwrap_or(0)),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    design.image.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_user(user: &GitlabUser) -> String {
    if let Some(username) = &user.username {
        return format!("@{username}");
    }
    if let Some(name) = &user.name {
        return name.clone();
    }
    user.id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

pub(crate) fn render_releases_text(
    releases: &[ReleaseResponse],
    formatter: TextFormatter,
) -> String {
    if releases.is_empty() {
        return String::new();
    }
    releases
        .iter()
        .map(|release| render_release_line(release, formatter))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub(crate) fn render_release_text(release: &ReleaseResponse, formatter: TextFormatter) -> String {
    render_release_line(release, formatter) + "\n"
}

pub(crate) fn render_release_line(release: &ReleaseResponse, formatter: TextFormatter) -> String {
    format!(
        "{} {} {}",
        formatter.paint(TextStyle::Key, &release.tag_name),
        release.name.as_deref().unwrap_or("-"),
        formatter.paint(
            TextStyle::Muted,
            release.released_at.as_deref().unwrap_or("-")
        )
    )
}

pub(crate) fn render_pipelines_text(
    pipelines: &[PipelineResponse],
    formatter: TextFormatter,
) -> String {
    if pipelines.is_empty() {
        return String::new();
    }
    pipelines
        .iter()
        .map(|pipeline| {
            format!(
                "{} {} {} {}",
                formatter.paint(TextStyle::Key, pipeline.id),
                formatter.paint(TextStyle::Key, pipeline.r#ref.as_deref().unwrap_or("-")),
                formatter.paint(execution_status_style(&pipeline.status), &pipeline.status),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    pipeline.web_url.as_deref().unwrap_or("")
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
            format!(
                "{} {} {} {}",
                formatter.paint(TextStyle::Key, job.id),
                formatter.paint(TextStyle::Key, &job.name),
                formatter.paint(execution_status_style(&job.status), &job.status),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    job.web_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// Deliberately not shared with the GitHub plugin's equivalent.
///
/// The two tables have the same shape and different vocabularies - `open` vs
/// `opened`, `cancelled` vs `canceled`, non-overlapping pending sets. Merging
/// them would either claim a state one forge does not have or drop styling the
/// other does, and the genuinely common part is a six-line `match`.
pub(crate) fn issue_state_style(state: &str) -> TextStyle {
    match state {
        "opened" | "open" => TextStyle::Success,
        "closed" => TextStyle::Muted,
        _ => TextStyle::Warning,
    }
}

pub(crate) fn execution_status_style(status: &str) -> TextStyle {
    match status {
        "success" | "passed" | "active" => TextStyle::Success,
        "failed" | "failure" | "canceled" | "cancelled" | "timed_out" => TextStyle::Error,
        "created"
        | "waiting_for_resource"
        | "preparing"
        | "pending"
        | "running"
        | "scheduled"
        | "manual" => TextStyle::Warning,
        "skipped" | "-" => TextStyle::Muted,
        _ => TextStyle::Muted,
    }
}

pub(crate) fn optional_value_style(value: &str) -> TextStyle {
    if value == "-" {
        TextStyle::Muted
    } else {
        TextStyle::Key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_renderer_preserves_plain_contract_and_styles_metadata() {
        let issue = issue_fixture();

        assert_eq!(
            render_issues_text(
                std::slice::from_ref(&issue),
                TextFormatter::with_color(false)
            ),
            "#42 opened Fix formatter https://gitlab.example/acme/tool/-/issues/42\n"
        );

        let rendered = render_issues_text(&[issue], TextFormatter::with_color(true));
        assert!(rendered.contains("#\u{1b}[36m42\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[32mopened\u{1b}[0m"));
        assert!(rendered.contains("Fix formatter"));
        assert!(!rendered.contains("\u{1b}[0mFix formatter"));
    }

    #[test]
    fn full_issue_renderer_keeps_description_and_comment_bodies_raw() {
        let issue = issue_fixture();
        let comments = vec![IssueNoteResponse {
            id: 9,
            body: Some("raw comment body".to_owned()),
            author: Some(GitlabUser {
                id: Some(1),
                username: Some("alice".to_owned()),
                name: None,
            }),
            created_at: Some("2026-07-16".to_owned()),
            updated_at: None,
            system: Some(false),
            web_url: None,
        }];
        let warnings = vec!["designs unavailable".to_owned()];

        let plain = render_issue_full_text(
            &issue,
            &comments,
            &[],
            &warnings,
            TextFormatter::with_color(false),
        );
        let styled = render_issue_full_text(
            &issue,
            &comments,
            &[],
            &warnings,
            TextFormatter::with_color(true),
        );

        assert_eq!(render::strip_ansi_sequences(&styled), plain);
        assert!(styled.contains("\nraw description\n"));
        assert!(styled.contains("\nraw comment body\n"));
        assert!(!styled.contains("\u{1b}[36mraw description"));
        assert!(!styled.contains("\u{1b}[36mraw comment body"));
        assert!(styled.contains("\u{1b}[33mwarnings:\u{1b}[0m"));
    }

    #[test]
    fn pipeline_renderer_maps_failed_status_to_error() {
        let pipeline = PipelineResponse {
            id: 7,
            iid: None,
            project_id: Some(1),
            sha: Some("abc123".to_owned()),
            r#ref: Some("main".to_owned()),
            status: "failed".to_owned(),
            source: Some("push".to_owned()),
            web_url: Some("https://gitlab.example/acme/tool/-/pipelines/7".to_owned()),
            created_at: None,
            updated_at: None,
        };

        assert_eq!(
            render_pipelines_text(
                std::slice::from_ref(&pipeline),
                TextFormatter::with_color(false)
            ),
            "7 main failed https://gitlab.example/acme/tool/-/pipelines/7\n"
        );

        let rendered = render_pipelines_text(&[pipeline], TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36m7\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mmain\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;31mfailed\u{1b}[0m"));
    }

    fn issue_fixture() -> IssueResponse {
        IssueResponse {
            id: 100,
            iid: 42,
            project_id: Some(1),
            title: "Fix formatter".to_owned(),
            description: Some("raw description".to_owned()),
            state: "opened".to_owned(),
            web_url: Some("https://gitlab.example/acme/tool/-/issues/42".to_owned()),
            author: Some(GitlabUser {
                id: Some(1),
                username: Some("alice".to_owned()),
                name: None,
            }),
            assignees: Some(Vec::new()),
            labels: vec!["bug".to_owned()],
            created_at: Some("2026-07-16".to_owned()),
            updated_at: Some("2026-07-16".to_owned()),
            closed_at: None,
        }
    }
}
