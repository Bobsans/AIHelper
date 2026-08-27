//! Rendering a finished `ai` report, and the shared vocabulary for naming and
//! colouring an action.

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions, TextStyle};

use super::rules::BlockAction as Action;

use super::install::*;

pub(super) fn action_label(action: Action) -> &'static str {
    match action {
        Action::Installed => "installed",
        Action::Updated => "updated",
        Action::Unchanged => "unchanged",
        Action::Removed => "removed",
        Action::NotPresent => "not present",
        Action::Skipped => "skipped",
    }
}

pub(super) fn action_style(action: Action) -> TextStyle {
    match action {
        Action::Installed | Action::Updated | Action::Removed => TextStyle::Success,
        Action::Unchanged | Action::NotPresent | Action::Skipped => TextStyle::Muted,
    }
}

pub(super) fn status_action_label(action: StatusAction) -> &'static str {
    match action {
        StatusAction::Installed => "installed",
        StatusAction::NotPresent => "not present",
        StatusAction::Unknown => "unknown",
    }
}

pub(super) fn status_action_style(action: StatusAction) -> TextStyle {
    match action {
        StatusAction::Installed => TextStyle::Success,
        StatusAction::NotPresent | StatusAction::Unknown => TextStyle::Muted,
    }
}

pub(super) fn emit(report: &TargetReport, options: &GlobalOptions) -> Result<(), AppError> {
    let mut emitter = Emitter::stdio(options);
    for warning in &report.warnings {
        emitter.warning(warning);
    }
    emitter.value(report, |formatter| {
        let prefix = if report.dry_run { "would be " } else { "" };
        let mut detail = format!("scope {}", report.mcp.scope.as_str());
        if let Some(transport) = report.mcp.transport {
            detail = format!("{}, {detail}", transport.as_str());
        }
        let mut lines = vec![
            format!(
                "{} mcp {}{} ({detail})",
                formatter.paint(TextStyle::Heading, report.target),
                prefix,
                formatter.paint(
                    action_style(report.mcp.action),
                    action_label(report.mcp.action)
                ),
            ),
            format!(
                "{} rules {}{} ({})",
                formatter.paint(TextStyle::Heading, report.target),
                prefix,
                formatter.paint(
                    action_style(report.rules.action),
                    action_label(report.rules.action)
                ),
                formatter.paint(TextStyle::Key, &report.rules.path),
            ),
        ];
        if let Some(managed) = &report.managed_service {
            lines.push(format!(
                "{} managed service {}{} ({})",
                formatter.paint(TextStyle::Heading, report.target),
                prefix,
                formatter.paint(TextStyle::Success, managed.action.as_str()),
                formatter.paint(TextStyle::Key, &managed.endpoint),
            ));
        }
        lines.extend(report.mcp.commands.iter().map(|invocation| {
            format!(
                "  {} {}",
                formatter.paint(
                    TextStyle::Muted,
                    if report.dry_run { "would run" } else { "ran" }
                ),
                formatter.paint(TextStyle::Muted, invocation),
            )
        }));
        lines.join(
            "
",
        )
    })
}

pub(super) fn emit_status(report: &StatusReport, options: &GlobalOptions) -> Result<(), AppError> {
    Emitter::stdio(options).value(report, |formatter| {
        let mut lines = Vec::new();
        for entry in &report.targets {
            let cli_state = match entry.cli {
                Some(cli) if entry.cli_available => {
                    formatter.paint(TextStyle::Success, format!("`{cli}` available"))
                }
                Some(cli) => formatter.paint(TextStyle::Warning, format!("`{cli}` missing")),
                None => formatter.paint(TextStyle::Muted, "config file"),
            };
            lines.push(format!(
                "{} ({cli_state})",
                formatter.paint(TextStyle::Heading, entry.target)
            ));
            if let Some(legacy) = entry.legacy_server {
                lines.push(format!(
                    "  {}",
                    formatter.paint(
                        TextStyle::Warning,
                        format!(
                            "legacy `{legacy}` registration present;                                  `ah ai install` or `ah ai uninstall` removes it"
                        )
                    )
                ));
            }
            for scope in &entry.scopes {
                lines.push(format!(
                    "  {}",
                    formatter.paint(TextStyle::Key, scope.scope.as_str())
                ));
                lines.push(match &scope.mcp {
                    Some(mcp) => {
                        let detail = mcp
                            .transport
                            .map(|transport| transport.as_str())
                            .or(mcp.detail.as_deref())
                            .map(|detail| format!(" ({detail})"))
                            .unwrap_or_default();
                        format!(
                            "    mcp   {}{detail}",
                            formatter.paint(
                                status_action_style(mcp.action),
                                status_action_label(mcp.action)
                            ),
                        )
                    }
                    None => format!(
                        "    mcp   {}",
                        formatter.paint(TextStyle::Muted, "not supported")
                    ),
                });
                lines.push(match &scope.rules {
                    Some(rules) => {
                        let detail = rules
                            .path
                            .as_deref()
                            .or(rules.detail.as_deref())
                            .map(|detail| format!(" ({})", formatter.paint(TextStyle::Key, detail)))
                            .unwrap_or_default();
                        format!(
                            "    rules {}{detail}",
                            formatter.paint(
                                status_action_style(rules.action),
                                status_action_label(rules.action)
                            ),
                        )
                    }
                    None => format!(
                        "    rules {}",
                        formatter.paint(TextStyle::Muted, "not supported")
                    ),
                });
            }
        }
        lines.join("\n")
    })
}
