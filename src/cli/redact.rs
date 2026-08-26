//! Redacting argv before it reaches the event log.
//!
//! A secret's value arrives on the command line, so the log has to be told
//! which position holds it.

use super::*;

pub(crate) fn redact_secret_command_argv(raw_args: &[OsString]) -> Vec<OsString> {
    let mut sanitized = raw_args.to_vec();
    let Some(action_index) = secret_action_index(&sanitized) else {
        return sanitized;
    };
    let add = sanitized[action_index] == "add";
    let mut id_seen = false;
    let mut preserve_next = false;
    let mut redact_next = false;
    let mut positional_only = false;

    for argument in sanitized.iter_mut().skip(action_index + 1) {
        let value = argument.to_string_lossy();
        if preserve_next {
            preserve_next = false;
            continue;
        }
        // An unknown option may carry a secret, so its argument is redacted even
        // when the value itself looks like an option.
        if redact_next {
            *argument = OsString::from(ah_redact::REDACTED);
            redact_next = false;
            continue;
        }

        if !positional_only && value == "--" {
            positional_only = true;
            continue;
        }
        if !positional_only && value.starts_with('-') {
            let body = value.trim_start_matches('-');
            let (name, assigned) = body
                .split_once('=')
                .map_or((body, false), |(name, _)| (name, true));
            let known_value =
                matches!(name, "label" | "description" | "cwd" | "limit") || add && name == "kind";
            if known_value {
                preserve_next = !assigned;
            } else if assigned {
                *argument = OsString::from(format!("--{name}={}", ah_redact::REDACTED));
            } else if !matches!(name, "open" | "json" | "quiet" | "help") {
                redact_next = true;
            }
            continue;
        }
        if id_seen {
            *argument = OsString::from(ah_redact::REDACTED);
        } else {
            id_seen = true;
        }
    }
    sanitized
}

pub(super) fn secret_action_index(raw_args: &[OsString]) -> Option<usize> {
    let mut index = 1;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != HOST_COMMAND_SECRETS {
        return None;
    }
    index += 1;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        return matches!(raw_args[index].to_str(), Some("add" | "edit")).then_some(index);
    }
    None
}
