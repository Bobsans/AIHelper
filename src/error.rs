use std::{ffi::OsStr, io, path::PathBuf};

use ah_plugin_api::ErrorDiagnostic;
use thiserror::Error;

use crate::output::{TextFormatter, TextStyle};

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{message}")]
    External { code: String, message: String },
    #[error("unknown command domain: {command}")]
    UnknownCommand {
        command: String,
        suggestion: Option<CommandSuggestion>,
    },
    #[error("{source}")]
    SuggestionContext {
        source: Box<AppError>,
        suggestion_description: Option<String>,
        follow_up: Option<FollowUpSuggestion>,
    },
    #[error("{diagnostic}")]
    Diagnostic { diagnostic: ErrorDiagnostic },
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("failed to change working directory to {path:?}: {source}")]
    ChangeDirectory {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read file {path:?}: {source}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write file {path:?}: {source}")]
    FileWrite {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to read file metadata for {path:?}: {source}")]
    FileMetadata {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to list directory {path:?}: {source}")]
    DirectoryRead {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to execute command '{command}': {source}")]
    CommandExecution {
        command: String,
        #[source]
        source: io::Error,
    },
    #[error("command failed '{command}' (code: {code:?}): {stderr}")]
    CommandFailed {
        command: String,
        code: Option<i32>,
        stderr: String,
    },
    #[error("failed to parse json file {path:?}: {source}")]
    JsonDeserialization {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("failed to serialize json output: {0}")]
    JsonSerialization(#[from] serde_json::Error),
}

impl AppError {
    pub fn code(&self) -> &str {
        match self {
            Self::External { code, .. } => code.as_str(),
            Self::UnknownCommand { .. } => "DOMAIN_NOT_FOUND",
            Self::SuggestionContext { source, .. } => source.code(),
            Self::Diagnostic { diagnostic } => diagnostic.code.as_str(),
            Self::InvalidArgument(message) => {
                classify_invalid_argument(&normalize_message(message))
            }
            Self::ChangeDirectory { .. } => "CWD_CHANGE_FAILED",
            Self::FileRead { source, .. } => {
                if source.kind() == io::ErrorKind::NotFound {
                    "FILE_NOT_FOUND"
                } else {
                    "FILE_READ_FAILED"
                }
            }
            Self::FileWrite { .. } => "FILE_WRITE_FAILED",
            Self::FileMetadata { source, .. } => {
                if source.kind() == io::ErrorKind::NotFound {
                    "FILE_NOT_FOUND"
                } else {
                    "FILE_METADATA_FAILED"
                }
            }
            Self::DirectoryRead { source, .. } => {
                if source.kind() == io::ErrorKind::NotFound {
                    "DIRECTORY_NOT_FOUND"
                } else {
                    "DIRECTORY_READ_FAILED"
                }
            }
            Self::CommandExecution { .. } => "COMMAND_EXECUTION_FAILED",
            Self::CommandFailed { .. } => "COMMAND_FAILED",
            Self::JsonDeserialization { .. } => "JSON_DESERIALIZATION_FAILED",
            Self::JsonSerialization(_) => "JSON_SERIALIZATION_FAILED",
        }
    }

    pub fn exit_code(&self) -> i32 {
        1
    }

    pub fn print(&self) {
        if wants_json_error_output() {
            match serde_json::to_string_pretty(&self.diagnostic()) {
                Ok(payload) => eprintln!("{payload}"),
                Err(error) => eprintln!("JSON_SERIALIZATION_FAILED: {error}"),
            }
            return;
        }

        let rendered = self.rendered();
        let formatter = TextFormatter::stderr();
        let invocation = std::env::args().skip(1).collect::<Vec<_>>();
        eprintln!(
            "{}",
            render_console_diagnostic(&self.console_diagnostic(&rendered, &invocation), formatter)
        );
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
    }

    pub fn external(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::External {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unknown_command(
        command: impl Into<String>,
        suggestion: Option<CommandSuggestion>,
    ) -> Self {
        Self::UnknownCommand {
            command: command.into(),
            suggestion,
        }
    }

    pub fn with_suggestion_context(
        self,
        suggestion_description: Option<String>,
        follow_up: Option<FollowUpSuggestion>,
    ) -> Self {
        if suggestion_description.is_none() && follow_up.is_none() {
            return self;
        }
        Self::SuggestionContext {
            source: Box::new(self),
            suggestion_description,
            follow_up,
        }
    }

    pub fn from_diagnostic(diagnostic: ErrorDiagnostic) -> Self {
        Self::Diagnostic { diagnostic }
    }

    pub fn cwd(path: PathBuf, source: io::Error) -> Self {
        Self::ChangeDirectory { path, source }
    }

    pub fn file_read(path: PathBuf, source: io::Error) -> Self {
        Self::FileRead { path, source }
    }

    pub fn file_write(path: PathBuf, source: io::Error) -> Self {
        Self::FileWrite { path, source }
    }

    pub fn file_metadata(path: PathBuf, source: io::Error) -> Self {
        Self::FileMetadata { path, source }
    }

    pub fn directory_read(path: PathBuf, source: io::Error) -> Self {
        Self::DirectoryRead { path, source }
    }

    pub fn command_execution(command: impl Into<String>, source: io::Error) -> Self {
        Self::CommandExecution {
            command: command.into(),
            source,
        }
    }

    pub fn command_failed(
        command: impl Into<String>,
        code: Option<i32>,
        stderr: impl Into<String>,
    ) -> Self {
        Self::CommandFailed {
            command: command.into(),
            code,
            stderr: stderr.into(),
        }
    }

    pub fn json_deserialization(path: PathBuf, source: serde_json::Error) -> Self {
        Self::JsonDeserialization { path, source }
    }

    pub fn user_message(&self) -> String {
        self.rendered().message
    }

    pub fn detail_message(&self) -> String {
        match self {
            Self::External { message, .. } => normalize_message(message),
            Self::UnknownCommand { command, .. } => format!("unknown command domain: {command}"),
            Self::SuggestionContext { source, .. } => source.detail_message(),
            Self::Diagnostic { diagnostic } => normalize_message(&diagnostic.cause),
            Self::InvalidArgument(message) => normalize_message(message),
            Self::ChangeDirectory { path, source } => {
                format!(
                    "failed to change working directory '{}': {source}",
                    path.display()
                )
            }
            Self::FileRead { path, source } => {
                format!("failed to read file '{}': {source}", path.display())
            }
            Self::FileWrite { path, source } => {
                format!("failed to write file '{}': {source}", path.display())
            }
            Self::FileMetadata { path, source } => {
                format!(
                    "failed to read file metadata '{}': {source}",
                    path.display()
                )
            }
            Self::DirectoryRead { path, source } => {
                format!("failed to read directory '{}': {source}", path.display())
            }
            Self::CommandExecution { command, source } => {
                format!("failed to execute command '{command}': {source}")
            }
            Self::CommandFailed {
                command,
                code,
                stderr,
            } => format!(
                "command failed '{command}' (code: {:?}): {}",
                code,
                stderr.trim()
            ),
            Self::JsonDeserialization { path, source } => {
                format!("failed to parse json file '{}': {source}", path.display())
            }
            Self::JsonSerialization(source) => format!("failed to serialize json output: {source}"),
        }
    }

    fn rendered(&self) -> RenderedError {
        match self {
            Self::External { code, message } => render_external(code, message),
            Self::UnknownCommand { command, .. } => RenderedError {
                code: "DOMAIN_NOT_FOUND".to_owned(),
                message: format!("unknown command domain: {command}"),
                context: Vec::new(),
            },
            Self::SuggestionContext { source, .. } => source.rendered(),
            Self::Diagnostic { diagnostic } => RenderedError {
                code: diagnostic.code.clone(),
                message: normalize_message(&diagnostic.message),
                context: Vec::new(),
            },
            Self::InvalidArgument(message) => render_invalid_argument(message),
            Self::ChangeDirectory { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to change working directory".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::FileRead { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to read file".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::FileWrite { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to write file".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::FileMetadata { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to read file metadata".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::DirectoryRead { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to read directory".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::CommandExecution { command, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to execute external command".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "command",
                        value: command.clone(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::CommandFailed {
                command,
                code,
                stderr,
            } => RenderedError {
                code: self.code().to_owned(),
                message: "external command exited with failure status".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "command",
                        value: command.clone(),
                    },
                    RenderedContext {
                        label: "exit_code",
                        value: code
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "unknown".to_owned()),
                    },
                    RenderedContext {
                        label: "stderr",
                        value: stderr.trim().to_owned(),
                    },
                ],
            },
            Self::JsonDeserialization { path, source } => RenderedError {
                code: self.code().to_owned(),
                message: "failed to parse JSON file".to_owned(),
                context: vec![
                    RenderedContext {
                        label: "path",
                        value: path.to_string_lossy().into_owned(),
                    },
                    RenderedContext {
                        label: "reason",
                        value: source.to_string(),
                    },
                ],
            },
            Self::JsonSerialization(source) => RenderedError {
                code: self.code().to_owned(),
                message: "failed to serialize JSON output".to_owned(),
                context: vec![RenderedContext {
                    label: "reason",
                    value: source.to_string(),
                }],
            },
        }
    }

    pub fn diagnostic(&self) -> ErrorDiagnostic {
        match self {
            Self::Diagnostic { diagnostic } => diagnostic.clone(),
            Self::SuggestionContext { source, .. } => source.diagnostic(),
            _ => {
                let rendered = self.rendered();
                ErrorDiagnostic::new(
                    infer_domain(&rendered.code),
                    infer_operation(&rendered.code),
                    rendered.code.clone(),
                    rendered.message.clone(),
                    self.detail_message(),
                    self.exit_code(),
                )
            }
        }
    }

    fn console_diagnostic(
        &self,
        rendered: &RenderedError,
        invocation: &[String],
    ) -> ConsoleDiagnostic {
        if let Self::SuggestionContext {
            source,
            suggestion_description,
            follow_up,
        } = self
        {
            let source_rendered = source.rendered();
            let mut diagnostic = source.console_diagnostic(&source_rendered, invocation);
            if let Some(suggestion) = &mut diagnostic.suggestion {
                suggestion.description = suggestion_description.clone();
            }
            diagnostic.follow_up = follow_up.clone();
            return diagnostic;
        }
        if rendered.code == "INVALID_ARGUMENT" {
            return console_diagnostic_from_invalid_argument(&self.detail_message(), invocation);
        }
        match self {
            Self::UnknownCommand {
                command,
                suggestion,
            } => ConsoleDiagnostic {
                message: format!("'{command}' is not a command."),
                suggestion: suggestion.clone(),
                usage: vec!["ah <domain> <command> [options]".to_owned()],
                help_command: Some("ah --help".to_owned()),
                ..ConsoleDiagnostic::default()
            },
            Self::InvalidArgument(message) => {
                console_diagnostic_from_invalid_argument(message, invocation)
            }
            Self::Diagnostic { diagnostic } => {
                let message = normalize_message(&diagnostic.message);
                let cause = normalize_message(&diagnostic.cause);
                let (message, details) = if diagnostic.code == "REGEX_INVALID" {
                    (
                        format!(
                            "invalid regular expression: {}",
                            regex_error_summary(&cause)
                                .or_else(|| regex_error_summary(&message))
                                .unwrap_or_else(|| "invalid expression".to_owned())
                        ),
                        Vec::new(),
                    )
                } else {
                    let details = (cause != message && !cause.is_empty())
                        .then(|| format!("Reason: {cause}"))
                        .into_iter()
                        .collect();
                    (message, details)
                };
                ConsoleDiagnostic {
                    message,
                    details,
                    hints: human_hint(&diagnostic.code)
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    ..ConsoleDiagnostic::default()
                }
            }
            _ => ConsoleDiagnostic {
                message: human_error_message(self, rendered),
                hints: human_hint(&rendered.code)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                ..ConsoleDiagnostic::default()
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSuggestion {
    pub command: String,
    pub description: Option<String>,
}

impl CommandSuggestion {
    pub fn new(command: impl Into<String>, description: Option<String>) -> Self {
        Self {
            command: command.into(),
            description,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpSuggestion {
    pub label: String,
    pub suggestion: CommandSuggestion,
}

impl FollowUpSuggestion {
    pub fn new(label: impl Into<String>, suggestion: CommandSuggestion) -> Self {
        Self {
            label: label.into(),
            suggestion,
        }
    }
}

#[derive(Debug)]
struct RenderedError {
    code: String,
    message: String,
    context: Vec<RenderedContext>,
}

#[derive(Debug)]
struct RenderedContext {
    label: &'static str,
    value: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ConsoleDiagnostic {
    message: String,
    details: Vec<String>,
    suggestion: Option<CommandSuggestion>,
    follow_up: Option<FollowUpSuggestion>,
    usage: Vec<String>,
    help_command: Option<String>,
    hints: Vec<String>,
}

fn render_invalid_argument(raw: &str) -> RenderedError {
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

fn classify_invalid_argument(message: &str) -> &'static str {
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

fn concise_error_message(error: &RenderedError) -> String {
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

fn human_error_message(error: &AppError, rendered: &RenderedError) -> String {
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

fn human_hint(code: &str) -> Option<&'static str> {
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

fn console_diagnostic_from_invalid_argument(raw: &str, invocation: &[String]) -> ConsoleDiagnostic {
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

fn strip_clap_error_prefix(message: &str) -> String {
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

fn parse_clap_suggestion(line: &str) -> Option<&str> {
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

fn suggested_invocation(invocation: &[String], candidate: &str) -> String {
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

fn parse_usage_lines(lines: &[&str]) -> Vec<String> {
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

fn prefix_ah(usage: &str) -> String {
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

fn help_command_from_usage(usage: &str) -> Option<String> {
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

fn render_console_diagnostic(diagnostic: &ConsoleDiagnostic, formatter: TextFormatter) -> String {
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

fn wants_json_error_output() -> bool {
    std::env::args_os().any(|arg| arg == OsStr::new("--json"))
}

fn infer_domain(code: &str) -> Option<String> {
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

fn infer_operation(code: &str) -> Option<String> {
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

fn context_value(error: &RenderedError, label: &str) -> Option<String> {
    error
        .context
        .iter()
        .find(|context| context.label == label)
        .map(|context| context.value.clone())
}

fn regex_error_summary(message: &str) -> Option<String> {
    message.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("error:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn strip_after_colon(message: &str) -> Option<String> {
    message
        .rsplit_once(':')
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn compact_message(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_owned()
}

fn normalize_message(raw: &str) -> String {
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

fn strip_one_wrapper(message: &str) -> String {
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

fn strip_leading_code_tag(message: &str) -> Option<&str> {
    if !message.starts_with('[') {
        return None;
    }
    let end = message.find("] ")?;
    Some(&message[(end + 2)..])
}

fn render_external(code: &str, message: &str) -> RenderedError {
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

#[cfg(test)]
mod tests {
    use super::{
        AppError, CommandSuggestion, console_diagnostic_from_invalid_argument,
        render_console_diagnostic,
    };
    use crate::output::TextFormatter;

    #[test]
    fn unknown_command_rendering_is_actionable_without_internal_code() {
        let error = AppError::unknown_command(
            "version",
            Some(CommandSuggestion::new(
                "ah --version",
                Some("Show the AIHelper version".to_owned()),
            )),
        );
        let rendered = error.rendered();
        let diagnostic = error.console_diagnostic(&rendered, &["version".to_owned()]);
        let formatter = TextFormatter::with_color(false);

        assert_eq!(
            render_console_diagnostic(&diagnostic, formatter),
            "ah: 'version' is not a command.\n\nDid you mean:\n  ah --version    Show the AIHelper version\n\nUsage:\n  ah <domain> <command> [options]\n\nRun 'ah --help' for more information."
        );
    }

    #[test]
    fn operational_error_rendering_styles_human_labels() {
        let error = AppError::external("DOMAIN_DISABLED", "plugin domain is disabled: file");
        let rendered = error.rendered();
        let diagnostic = error.console_diagnostic(&rendered, &[]);
        let formatter = TextFormatter::with_color(true);

        assert_eq!(
            render_console_diagnostic(&diagnostic, formatter),
            "\u{1b}[1;31mah:\u{1b}[0m plugin domain is disabled: file\n\n\u{1b}[33mHint:\u{1b}[0m Enable the plugin domain or choose another command."
        );
    }

    #[test]
    fn clap_diagnostic_retains_suggestion_usage_and_scoped_help() {
        let diagnostic = console_diagnostic_from_invalid_argument(
            "error: unrecognized subcommand 'versoin'\n\n  tip: a similar subcommand exists: 'version'\n\nUsage: project <COMMAND>\n\nFor more information, try '--help'.",
            &["project".to_owned(), "versoin".to_owned()],
        );

        assert_eq!(diagnostic.message, "unrecognized subcommand 'versoin'.");
        assert_eq!(
            diagnostic
                .suggestion
                .as_ref()
                .map(|suggestion| suggestion.command.as_str()),
            Some("ah project version")
        );
        assert_eq!(diagnostic.usage, ["ah project <COMMAND>"]);
        assert_eq!(
            diagnostic.help_command.as_deref(),
            Some("ah project --help")
        );
    }

    #[test]
    fn clap_diagnostic_retains_missing_argument_name() {
        let diagnostic = console_diagnostic_from_invalid_argument(
            "error: the following required arguments were not provided:\n  <PATTERN>\n\nUsage: search text <PATTERN> [PATH]...\n\nFor more information, try '--help'.",
            &["search".to_owned(), "text".to_owned()],
        );

        assert_eq!(
            diagnostic.message,
            "the following required arguments were not provided:"
        );
        assert_eq!(diagnostic.details, ["<PATTERN>"]);
        assert_eq!(diagnostic.usage, ["ah search text <PATTERN> [PATH]..."]);
        assert_eq!(
            diagnostic.help_command.as_deref(),
            Some("ah search text --help")
        );
    }
}
