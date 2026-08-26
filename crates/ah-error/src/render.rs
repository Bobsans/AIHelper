//! Turning an `AppError` into what the user sees: the console diagnostic, the
//! JSON payload, and the hint that goes with a code.

use super::*;

#[derive(Debug)]
pub(super) struct RenderedError {
    pub(super) code: String,
    pub(super) message: String,
    pub(super) context: Vec<RenderedContext>,
}

#[derive(Debug)]
pub(super) struct RenderedContext {
    pub(super) label: &'static str,
    pub(super) value: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ConsoleDiagnostic {
    pub(super) message: String,
    pub(super) details: Vec<String>,
    pub(super) suggestion: Option<CommandSuggestion>,
    pub(super) follow_up: Option<FollowUpSuggestion>,
    pub(super) usage: Vec<String>,
    pub(super) help_command: Option<String>,
    pub(super) hints: Vec<String>,
}

pub(super) fn render_invalid_argument(raw: &str) -> RenderedError {
    let message = normalize_message(raw);

    if let Some(path) = message.strip_prefix("path does not exist: ") {
        let path_value = path.to_owned();
        return RenderedError {
            code: "PATH_NOT_FOUND".to_owned(),
            message: message.clone(),
            context: vec![RenderedContext {
                label: "path",
                value: path_value,
            }],
        };
    }
    if let Some(path) = message.strip_prefix("path is not a file or directory: ") {
        let path_value = path.to_owned();
        return RenderedError {
            code: "PATH_INVALID_TYPE".to_owned(),
            message: message.clone(),
            context: vec![RenderedContext {
                label: "path",
                value: path_value,
            }],
        };
    }
    if let Some(task) = message.strip_prefix("task not found: ") {
        let task_value = task.to_owned();
        return RenderedError {
            code: "TASK_NOT_FOUND".to_owned(),
            message: message.clone(),
            context: vec![RenderedContext {
                label: "task",
                value: task_value,
            }],
        };
    }

    let code = classify_invalid_argument(&message);
    RenderedError {
        code: code.to_owned(),
        message,
        context: Vec::new(),
    }
}

pub(super) fn classify_invalid_argument(message: &str) -> &'static str {
    if message.starts_with("path does not exist: ") {
        return "PATH_NOT_FOUND";
    }
    if message.starts_with("path is not a file or directory: ") {
        return "PATH_INVALID_TYPE";
    }
    if message.starts_with("task not found: ") {
        return "TASK_NOT_FOUND";
    }
    if message.contains("symlink traversal is disabled") {
        return "SYMLINK_TRAVERSAL_BLOCKED";
    }
    if message.starts_with("invalid regex pattern:") {
        return "REGEX_INVALID";
    }
    if message.starts_with("invalid --glob") || message.starts_with("invalid glob set:") {
        return "GLOB_INVALID";
    }
    if message.contains("must be >= 1") || message.contains("must be >= --from") {
        return "INVALID_RANGE";
    }
    "INVALID_ARGUMENT"
}

pub(super) fn concise_error_message(error: &RenderedError) -> String {
    match error.code.as_str() {
        "PATH_NOT_FOUND" | "PATH_INVALID_TYPE" | "FILE_NOT_FOUND" | "DIRECTORY_NOT_FOUND" => {
            context_value(error, "path").unwrap_or_else(|| compact_message(&error.message))
        }
        "TASK_NOT_FOUND" => {
            context_value(error, "task").unwrap_or_else(|| compact_message(&error.message))
        }
        "DOMAIN_DISABLED" => {
            strip_after_colon(&error.message).unwrap_or_else(|| compact_message(&error.message))
        }
        "DOMAIN_NOT_FOUND" => {
            strip_after_colon(&error.message).unwrap_or_else(|| compact_message(&error.message))
        }
        "REGEX_INVALID" => {
            regex_error_summary(&error.message).unwrap_or_else(|| "invalid regex".to_owned())
        }
        "SYMLINK_TRAVERSAL_BLOCKED" => "symlink blocked".to_owned(),
        "INVALID_RANGE" => compact_message(&error.message),
        "GLOB_INVALID" => compact_message(&error.message),
        _ => compact_message(&error.message),
    }
}

pub(super) fn human_error_message(error: &AppError, rendered: &RenderedError) -> String {
    match rendered.code.as_str() {
        "REGEX_INVALID" => format!(
            "invalid regular expression: {}",
            concise_error_message(rendered)
        ),
        "PATH_NOT_FOUND" | "PATH_INVALID_TYPE" | "TASK_NOT_FOUND" | "DOMAIN_DISABLED" => {
            rendered.message.clone()
        }
        _ => error.detail_message(),
    }
}

pub(super) fn human_hint(code: &str) -> Option<&'static str> {
    match code {
        "PATH_NOT_FOUND" | "PATH_INVALID_TYPE" | "FILE_NOT_FOUND" | "DIRECTORY_NOT_FOUND" => {
            Some("Check the path or set a different working directory with --cwd.")
        }
        "REGEX_INVALID" => Some("Fix the expression or remove --regex to search literally."),
        "SYMLINK_TRAVERSAL_BLOCKED" => {
            Some("Use --follow-symlinks if following this link is intentional.")
        }
        "TASK_NOT_FOUND" => Some("Run 'ah task list' to see saved tasks."),
        "DOMAIN_NOT_FOUND" => Some("Run 'ah --help' to see available commands."),
        "DOMAIN_DISABLED" => Some("Enable the plugin domain or choose another command."),
        "DEPENDENCY_MISSING" => {
            Some("Install the required tool and make sure it is available on PATH.")
        }
        _ => None,
    }
}

pub(super) fn console_diagnostic_from_invalid_argument(
    raw: &str,
    invocation: &[String],
) -> ConsoleDiagnostic {
    let message = normalize_message(raw);
    let lines = message.lines().map(str::trim_end).collect::<Vec<_>>();
    let usage_index = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Usage:"));
    let suggestion_name = lines.iter().find_map(|line| parse_clap_suggestion(line));
    let suggestion = suggestion_name
        .map(|candidate| CommandSuggestion::new(suggested_invocation(invocation, candidate), None));

    if let Some(index) = usage_index {
        let usage = parse_usage_lines(&lines[index..]);
        let before_usage = &lines[..index];
        let mut content = before_usage
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with("tip:"))
            .collect::<Vec<_>>();
        let primary = if content.is_empty() {
            "a command is required.".to_owned()
        } else {
            strip_clap_error_prefix(content.remove(0))
        };
        let details = content.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let help_command = usage.first().and_then(|line| help_command_from_usage(line));
        return ConsoleDiagnostic {
            message: primary,
            details,
            suggestion,
            follow_up: None,
            usage,
            help_command,
            hints: Vec::new(),
        };
    }

    if let Some(scope) = message
        .strip_prefix("missing ")
        .and_then(|rest| rest.strip_suffix(" subcommand"))
    {
        return ConsoleDiagnostic {
            message: format!("a subcommand is required for 'ah {scope}'."),
            usage: vec![format!("ah {scope} <COMMAND>")],
            help_command: Some(format!("ah {scope} --help")),
            ..ConsoleDiagnostic::default()
        };
    }

    let rendered = render_invalid_argument(&message);
    ConsoleDiagnostic {
        message: match rendered.code.as_str() {
            "REGEX_INVALID" => {
                format!(
                    "invalid regular expression: {}",
                    concise_error_message(&rendered)
                )
            }
            _ => message,
        },
        hints: human_hint(&rendered.code)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ..ConsoleDiagnostic::default()
    }
}

pub(super) fn render_console_diagnostic(
    diagnostic: &ConsoleDiagnostic,
    formatter: TextFormatter,
) -> String {
    let mut output = format!(
        "{} {}",
        formatter.paint(TextStyle::Error, "ah:"),
        diagnostic.message
    );
    for detail in &diagnostic.details {
        output.push_str("\n  ");
        output.push_str(detail);
    }
    if let Some(suggestion) = &diagnostic.suggestion {
        output.push_str("\n\n");
        output.push_str(&formatter.paint(TextStyle::Heading, "Did you mean:"));
        output.push_str("\n  ");
        output.push_str(&suggestion.command);
        if let Some(description) = &suggestion.description {
            output.push_str("    ");
            output.push_str(description);
        }
    }
    if !diagnostic.usage.is_empty() {
        output.push_str("\n\n");
        output.push_str(&formatter.paint(TextStyle::Heading, "Usage:"));
        for usage in &diagnostic.usage {
            output.push_str("\n  ");
            output.push_str(usage);
        }
    }
    if let Some(follow_up) = &diagnostic.follow_up {
        output.push_str("\n\n");
        output.push_str(&follow_up.label);
        output.push_str("\n  ");
        output.push_str(&follow_up.suggestion.command);
        if let Some(description) = &follow_up.suggestion.description {
            output.push_str("    ");
            output.push_str(description);
        }
    }
    if let Some(help_command) = &diagnostic.help_command {
        output.push_str("\n\nRun '");
        output.push_str(help_command);
        output.push_str("' for more information.");
    }
    for hint in &diagnostic.hints {
        output.push_str("\n\n");
        output.push_str(&format!(
            "{} {hint}",
            formatter.paint(TextStyle::Warning, "Hint:")
        ));
    }
    output
}

pub(super) fn wants_json_error_output() -> bool {
    std::env::args_os().any(|arg| arg == OsStr::new("--json"))
}

pub(super) fn infer_domain(code: &str) -> Option<String> {
    let domain = if code.starts_with("GITHUB_") {
        "github"
    } else if code.starts_with("GITLAB_") {
        "gitlab"
    } else if code.starts_with("OLLAMA_") {
        "ollama"
    } else if code.starts_with("POSTGRES_") {
        "postgres"
    } else if code.starts_with("PLUGIN_") || code.starts_with("DOMAIN_") {
        "plugins"
    } else {
        return None;
    };
    Some(domain.to_owned())
}

pub(super) fn infer_operation(code: &str) -> Option<String> {
    let operation = match code {
        "COMMAND_EXECUTION_FAILED" | "COMMAND_FAILED" => "process.execute",
        "JSON_DESERIALIZATION_FAILED" | "JSON_SERIALIZATION_FAILED" => "json",
        "CWD_CHANGE_FAILED" => "cwd.change",
        "FILE_NOT_FOUND" | "FILE_READ_FAILED" | "FILE_WRITE_FAILED" | "FILE_METADATA_FAILED" => {
            "file"
        }
        "DIRECTORY_NOT_FOUND" | "DIRECTORY_READ_FAILED" => "directory",
        "DEPENDENCY_MISSING" => "dependency.preflight",
        _ if code.starts_with("PLUGIN_") || code.starts_with("DOMAIN_") => "plugin.runtime",
        _ => return None,
    };
    Some(operation.to_owned())
}

pub(super) fn context_value(error: &RenderedError, label: &str) -> Option<String> {
    error
        .context
        .iter()
        .find(|context| context.label == label)
        .map(|context| context.value.clone())
}

pub(super) fn render_external(code: &str, message: &str) -> RenderedError {
    let normalized_message = normalize_message(message);
    if code == "INVALID_ARGUMENT" {
        return render_invalid_argument(&normalized_message);
    }
    if code == "PATH_NOT_FOUND" || code == "PATH_INVALID_TYPE" || code == "TASK_NOT_FOUND" {
        return render_invalid_argument(&normalized_message);
    }
    RenderedError {
        code: code.to_owned(),
        message: normalized_message,
        context: Vec::new(),
    }
}
