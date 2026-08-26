//! Text-output primitives every plugin needs before it can render anything.
//!
//! These are the pieces that were copied rather than shared: answering an
//! invocation in the format the caller asked for, bounding a quoted fragment in
//! an error message, and stripping terminal control sequences out of captured
//! build output before it is shown or matched against.

use ah_plugin_api::{GlobalOptionsWire, InvocationResponse, TextFormatter, TextStyle};
use serde::Serialize;

/// Answer a successful invocation in the format the caller asked for.
///
/// `--quiet` wins over `--json`: a caller that asked for no output gets none,
/// even in JSON mode.
pub fn render_success<T: Serialize>(
    globals: &GlobalOptionsWire,
    output: &T,
    text_output: String,
) -> InvocationResponse {
    if globals.quiet {
        return InvocationResponse::ok(None);
    }
    if globals.json {
        match serde_json::to_string_pretty(output) {
            Ok(payload) => InvocationResponse::ok(Some(payload)),
            Err(error) => InvocationResponse::error(
                "JSON_SERIALIZATION_FAILED",
                format!("failed to serialize plugin output: {error}"),
            ),
        }
    } else {
        InvocationResponse::ok(Some(text_output))
    }
}

/// Bound a fragment quoted back in an error message, counting characters rather
/// than bytes so the cut never lands inside one.
pub fn truncate_for_error(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect::<String>() + "..."
}

/// Style `value`, or return an empty string rather than an empty styled string,
/// which would otherwise emit escape codes around nothing.
pub fn paint_if_present(formatter: TextFormatter, style: TextStyle, value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        formatter.paint(style, value)
    }
}

/// Remove terminal control sequences from captured output.
///
/// CI logs carry colour (CSI, `ESC [ … letter`) and window titles (OSC,
/// `ESC ] … BEL` or `ESC ] … ESC \`). Both have to go before the text is shown
/// or scanned for warnings, or a warning wrapped in colour will not match.
pub fn strip_ansi_sequences(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            output.push(ch);
            continue;
        }
        match chars.peek() {
            Some(&'[') => {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(&']') => {
                chars.next();
                loop {
                    match chars.next() {
                        None | Some('\x07') => break,
                        Some('\u{1b}') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_wins_over_json() {
        let globals = GlobalOptionsWire {
            json: true,
            quiet: true,
            limit: None,
            cwd: None,
        };
        assert_eq!(
            render_success(&globals, &"payload", "text".to_owned()).message,
            None
        );
    }

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate_for_error("héllo", 5), "héllo");
        assert_eq!(truncate_for_error("héllo", 3), "hél...");
    }

    #[test]
    fn colour_and_window_titles_are_both_stripped() {
        assert_eq!(
            strip_ansi_sequences("\u{1b}[1mDownloaded\u{1b}[0m"),
            "Downloaded"
        );
        assert_eq!(
            strip_ansi_sequences("\u{1b}]0;build\u{7}warning: x"),
            "warning: x"
        );
        assert_eq!(
            strip_ansi_sequences("\u{1b}]0;build\u{1b}\\warning: x"),
            "warning: x"
        );
    }

    #[test]
    fn an_empty_value_is_not_wrapped_in_escape_codes() {
        assert_eq!(
            paint_if_present(TextFormatter::with_color(true), TextStyle::Key, ""),
            ""
        );
    }
}
