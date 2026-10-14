//! What `ah upgrade` prints.
//!
//! The mechanism returns an outcome and this renders it. It used to call
//! `Emitter` from inside the activation, which is why `src/updater/` knew about
//! `GlobalOptions` at all - and a mechanism that renders cannot be tested
//! without deciding what its output should look like.

use ah_updater_core::{CheckStatus, UpdateOperation, UpgradeCheckResultV1};

use ah_output::{TextFormatter, TextStyle};
use ah_updater::{activate::UpgradeLaunchResult, check::UpgradeOutcome};

use crate::{cli::GlobalOptions, error::AppError, output::Emitter};

/// # Errors
///
/// [`AppError`] when the output cannot be written.
pub(crate) fn outcome(outcome: &UpgradeOutcome, options: GlobalOptions) -> Result<(), AppError> {
    match outcome {
        UpgradeOutcome::Checked(result) => check(result, options),
        UpgradeOutcome::Launched(result) => launch(result, options),
    }
}

fn check(result: &UpgradeCheckResultV1, options: GlobalOptions) -> Result<(), AppError> {
    Emitter::stdio(&options).value(result, |formatter| {
        check_message(
            check_status(result.status),
            &result.current_version,
            result.selected_version.as_deref().unwrap_or("unknown"),
            formatter,
        )
    })
}

fn launch(result: &UpgradeLaunchResult, options: GlobalOptions) -> Result<(), AppError> {
    Emitter::stdio(&options).value(result, |formatter| launch_message(result, formatter))
}

fn launch_message(result: &UpgradeLaunchResult, formatter: TextFormatter) -> String {
    if result.activation != "launched" {
        if result.operation == UpdateOperation::Version && result.status == "up_to_date" {
            return format!(
                "{} AIHelper {} is already installed.",
                formatter.paint(TextStyle::Success, "✓"),
                formatter.paint(TextStyle::Key, &result.current_version),
            );
        }
        return check_message(
            result.status,
            &result.current_version,
            &result.selected_version,
            formatter,
        );
    }
    let action = if result.operation == UpdateOperation::Rollback {
        "Rollback"
    } else {
        "Update"
    };
    let mut message = format!(
        "\n{} {} → {}\n\n{}",
        formatter.paint(TextStyle::Success, format!("✓ {action} started:")),
        formatter.paint(TextStyle::Key, &result.current_version),
        formatter.paint(TextStyle::Key, &result.selected_version),
        formatter.paint(
            TextStyle::Muted,
            "Installation will finish in the background."
        ),
    );
    if result.managed_mcp_restoration == "pending" {
        message.push('\n');
        message.push_str(&formatter.paint(
            TextStyle::Muted,
            "The MCP service will restart automatically.",
        ));
    }
    message
}

fn check_message(status: &str, current: &str, selected: &str, formatter: TextFormatter) -> String {
    let current = formatter.paint(TextStyle::Key, current);
    let selected = formatter.paint(TextStyle::Key, selected);
    match status {
        "up_to_date" => format!(
            "{} AIHelper {current} is up to date.",
            formatter.paint(TextStyle::Success, "✓")
        ),
        "current_newer" => {
            format!(
                "{}\n\n  Installed:       {current}\n  Latest release:  {selected}",
                formatter.paint(
                    TextStyle::Success,
                    "✓ You already have a newer version of AIHelper."
                )
            )
        }
        _ => format!(
            "{} AIHelper update available: {current} → {selected}\n\nRun {} to install it.",
            formatter.paint(TextStyle::Key, "↑"),
            formatter.paint(TextStyle::Key, "ah upgrade")
        ),
    }
}

fn check_status(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::UpToDate => "up_to_date",
        CheckStatus::UpdateAvailable => "update_available",
        CheckStatus::CurrentNewer => "current_newer",
    }
}

#[cfg(test)]
mod tests {
    use ah_output::OutputMode;

    use super::*;

    fn result() -> UpgradeLaunchResult {
        UpgradeLaunchResult {
            schema_version: 1,
            operation: UpdateOperation::Upgrade,
            status: "activation_launched",
            current_version: "2.0.0".into(),
            selected_version: "2.0.1".into(),
            target: "x86_64-pc-windows-msvc".into(),
            source: "github_release",
            activation: "launched",
            managed_mcp_restoration: "pending",
            rollback: "not_required",
        }
    }

    #[test]
    fn handoff_message_is_honest_and_mcp_line_is_conditional() {
        let mut result = result();
        assert_eq!(
            launch_message(&result, TextFormatter::with_color(false)),
            "\n✓ Update started: 2.0.0 → 2.0.1\n\nInstallation will finish in the background.\nThe MCP service will restart automatically."
        );
        result.managed_mcp_restoration = "not_required";
        assert!(!launch_message(&result, TextFormatter::with_color(false)).contains("MCP"));
        result.operation = UpdateOperation::Rollback;
        assert!(
            launch_message(&result, TextFormatter::with_color(false)).contains("Rollback started")
        );
        result.activation = "not_required";
        result.status = "up_to_date";
        assert_eq!(
            launch_message(&result, TextFormatter::with_color(false)),
            "✓ AIHelper 2.0.0 is up to date."
        );
        result.status = "current_newer";
        assert!(
            launch_message(&result, TextFormatter::with_color(false)).contains("newer version")
        );
    }

    #[test]
    fn availability_and_exact_version_messages_are_actionable() {
        let formatter = TextFormatter::with_color(false);
        assert_eq!(
            check_message("update_available", "2.0.0", "2.0.1", formatter),
            "↑ AIHelper update available: 2.0.0 → 2.0.1\n\nRun ah upgrade to install it."
        );
        let mut result = result();
        result.activation = "not_required";
        result.status = "up_to_date";
        result.operation = UpdateOperation::Version;
        assert_eq!(
            launch_message(&result, formatter),
            "✓ AIHelper 2.0.0 is already installed."
        );
    }

    #[test]
    fn color_only_changes_presentation() {
        let result = result();
        let colored = launch_message(&result, TextFormatter::with_color(true));
        assert_eq!(
            console::strip_ansi_codes(&colored),
            launch_message(&result, TextFormatter::with_color(false))
        );
        assert!(colored.contains("\u{1b}[32m✓ Update started:"));
        assert!(colored.contains("\u{1b}[36m2.0.1"));
    }

    #[test]
    fn json_preserves_launch_fields_and_quiet_suppresses_output() {
        let result = result();
        for quiet in [false, true] {
            let (mut emitter, captured) = Emitter::capture(&GlobalOptions {
                output: OutputMode::Json,
                quiet,
                limit: None,
                cwd: None,
            });
            emitter
                .value(&result, |_| panic!("JSON must not render text"))
                .unwrap();
            if quiet {
                assert_eq!(captured.stdout(), "");
            } else {
                let json: serde_json::Value = serde_json::from_str(&captured.stdout()).unwrap();
                assert_eq!(
                    json,
                    serde_json::json!({
                        "schema_version": 1, "operation": "upgrade", "status": "activation_launched",
                        "current_version": "2.0.0", "selected_version": "2.0.1",
                        "target": "x86_64-pc-windows-msvc", "source": "github_release",
                        "activation": "launched", "managed_mcp_restoration": "pending", "rollback": "not_required"
                    })
                );
            }
        }
    }
}
