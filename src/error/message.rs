//! Cleaning up the message text an error arrives with.
//!
//! clap, regex and the plugin boundary each wrap their messages differently,
//! and the diagnostic has to read the same whichever produced it.

use super::*;

pub(super) fn strip_clap_error_prefix(message: &str) -> String {
    let message = message
        .strip_prefix("error:")
        .unwrap_or(message)
        .trim()
        .to_owned();
    if message.ends_with(['.', ':', '?', '!']) {
        message
    } else {
        format!("{message}.")
    }
}

pub(super) fn parse_clap_suggestion(line: &str) -> Option<&str> {
    let line = line.trim();
    let suggestion = line
        .strip_prefix("tip: a similar subcommand exists:")?
        .trim();
    suggestion
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
}

pub(crate) fn suggested_subcommand(message: &str) -> Option<&str> {
    message.lines().find_map(parse_clap_suggestion)
}

pub(super) fn suggested_invocation(invocation: &[String], candidate: &str) -> String {
    let mut corrected = invocation.to_vec();
    if let Some(index) = corrected
        .iter()
        .rposition(|argument| !argument.starts_with('-'))
    {
        corrected[index] = candidate.to_owned();
    } else {
        corrected.push(candidate.to_owned());
    }
    format!("ah {}", corrected.join(" "))
}

pub(super) fn parse_usage_lines(lines: &[&str]) -> Vec<String> {
    let mut usage = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let value = if index == 0 {
            let Some(value) = trimmed.strip_prefix("Usage:") else {
                break;
            };
            value.trim()
        } else if line.starts_with(char::is_whitespace) && !trimmed.is_empty() {
            trimmed
        } else {
            break;
        };
        if value.is_empty() {
            continue;
        }
        usage.push(prefix_ah(value));
    }
    usage
}

pub(super) fn prefix_ah(usage: &str) -> String {
    if usage == "ah" || usage.starts_with("ah ") {
        usage.to_owned()
    } else if let Some((program, suffix)) = usage.split_once(' ')
        && PathBuf::from(program)
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("ah"))
    {
        format!("ah {suffix}")
    } else {
        format!("ah {usage}")
    }
}

pub(super) fn help_command_from_usage(usage: &str) -> Option<String> {
    let scope = usage
        .split_whitespace()
        .take_while(|token| {
            !token.starts_with('<')
                && !token.starts_with('[')
                && !token.starts_with('{')
                && !token.starts_with('-')
        })
        .collect::<Vec<_>>();
    if scope.is_empty() {
        return None;
    }
    Some(format!("{} --help", scope.join(" ")))
}

pub(super) fn regex_error_summary(message: &str) -> Option<String> {
    message.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("error:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

pub(super) fn strip_after_colon(message: &str) -> Option<String> {
    message
        .rsplit_once(':')
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn compact_message(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_owned()
}

pub(super) fn normalize_message(raw: &str) -> String {
    let mut message = raw.trim().to_owned();
    loop {
        let next = strip_one_wrapper(&message);
        if next == message {
            break;
        }
        message = next;
    }
    message
}

pub(super) fn strip_one_wrapper(message: &str) -> String {
    let trimmed = message.trim();

    if let Some(after_code) = strip_leading_code_tag(trimmed) {
        return after_code.to_owned();
    }

    for prefix in [
        "invalid argument: ",
        "plugin invocation failed: ",
        "plugin response parse failed: ",
    ] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim().to_owned();
        }
    }

    trimmed.to_owned()
}

pub(super) fn strip_leading_code_tag(message: &str) -> Option<&str> {
    if !message.starts_with('[') {
        return None;
    }
    let end = message.find("] ")?;
    Some(&message[(end + 2)..])
}
