use std::fmt::Display;

pub use ah_plugin_api::{TextFormatter, TextStyle};
use serde::Serialize;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutputMode {
    Text,
    Json,
}

pub(crate) fn render_semantic_count(
    label: &str,
    value: usize,
    non_zero_style: TextStyle,
    formatter: TextFormatter,
) -> String {
    formatter.paint(
        if value == 0 {
            TextStyle::Muted
        } else {
            non_zero_style
        },
        format!("{label}={value}"),
    )
}

pub(crate) fn git_status_style(status: &str) -> TextStyle {
    let normalized = status.trim();
    if normalized.contains('U') || normalized.contains('D') || normalized == "AA" {
        TextStyle::Error
    } else if normalized == "??" || normalized.contains('M') || normalized.contains('?') {
        TextStyle::Warning
    } else if normalized.contains('R') || normalized.contains('C') {
        TextStyle::Key
    } else if normalized.contains('A') {
        TextStyle::Success
    } else {
        TextStyle::Muted
    }
}

#[derive(Debug, Serialize)]
struct MessagePayload<'a> {
    command: &'a str,
    status: &'a str,
    message: &'a str,
}

pub fn emit_message(
    mode: OutputMode,
    quiet: bool,
    command: &str,
    message: &str,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }

    match mode {
        OutputMode::Text => println!("{message}"),
        OutputMode::Json => {
            let payload = MessagePayload {
                command,
                status: "ok",
                message,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

pub fn emit_not_implemented(mode: OutputMode, quiet: bool, command: &str) -> Result<(), AppError> {
    emit_message(
        mode,
        quiet,
        command,
        "This planned command is not implemented yet.",
    )
}

pub fn emit_warning(message: impl Display) {
    eprintln!("{}", render_warning_line(TextFormatter::stderr(), message));
}

pub fn emit_muted_stderr(message: impl Display) {
    eprintln!(
        "{}",
        TextFormatter::stderr().paint(TextStyle::Muted, message)
    );
}

fn render_warning_line(formatter: TextFormatter, message: impl Display) -> String {
    format!(
        "{} {message}",
        formatter.paint(TextStyle::Warning, "warning:")
    )
}

#[cfg(test)]
mod tests {
    use super::{
        TextFormatter, TextStyle, git_status_style, render_semantic_count, render_warning_line,
    };

    #[test]
    fn semantic_count_uses_muted_zero_and_requested_non_zero_style() {
        let formatter = TextFormatter::with_color(true);

        assert_eq!(
            render_semantic_count("changed", 0, TextStyle::Warning, formatter),
            "\u{1b}[2mchanged=0\u{1b}[0m"
        );
        assert_eq!(
            render_semantic_count("changed", 2, TextStyle::Warning, formatter),
            "\u{1b}[33mchanged=2\u{1b}[0m"
        );
    }

    #[test]
    fn git_status_style_maps_common_states() {
        assert_eq!(git_status_style("A "), TextStyle::Success);
        assert_eq!(git_status_style(" M"), TextStyle::Warning);
        assert_eq!(git_status_style("??"), TextStyle::Warning);
        assert_eq!(git_status_style("D "), TextStyle::Error);
        assert_eq!(git_status_style("UU"), TextStyle::Error);
        assert_eq!(git_status_style("AA"), TextStyle::Error);
        assert_eq!(git_status_style("R100"), TextStyle::Key);
    }

    #[test]
    fn warning_renderer_preserves_plain_contract() {
        assert_eq!(
            render_warning_line(TextFormatter::with_color(false), "output truncated"),
            "warning: output truncated"
        );
    }

    #[test]
    fn warning_renderer_styles_only_the_label() {
        assert_eq!(
            render_warning_line(TextFormatter::with_color(true), "output truncated"),
            "\u{1b}[33mwarning:\u{1b}[0m output truncated"
        );
    }
}
