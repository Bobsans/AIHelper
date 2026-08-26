//! `AppError`, and what a user sees when one reaches the surface.
//!
//! Its own crate because every subsystem returns it, which made it the thing
//! that kept them all inside the root crate. It depends on nothing of AIHelper
//! beyond the plugin ABI's text formatter, so the move was a relocation.
//!
//! The 1047 production lines this came from mixed the error type with the
//! console renderer and with thirteen small functions for cleaning up message
//! text, so the shape of the type was hard to see past the presentation:
//!
//! | Module    | Owns                                                        |
//! |-----------|-------------------------------------------------------------|
//! | `render`  | the console diagnostic, the JSON payload, the code hints     |
//! | `message` | normalising the message text clap, regex and plugins produce |

use std::{ffi::OsStr, io, path::PathBuf};

use ah_plugin_api::ErrorDiagnostic;
use thiserror::Error;

use ah_plugin_api::{TextFormatter, TextStyle};

mod message;
mod render;
pub use message::suggested_subcommand;
use message::{
    compact_message, help_command_from_usage, normalize_message, parse_clap_suggestion,
    parse_usage_lines, regex_error_summary, strip_after_colon, strip_clap_error_prefix,
    suggested_invocation,
};
use render::{
    ConsoleDiagnostic, RenderedContext, RenderedError, classify_invalid_argument,
    console_diagnostic_from_invalid_argument, human_error_message, human_hint, infer_domain,
    infer_operation, render_console_diagnostic, render_external, render_invalid_argument,
    wants_json_error_output,
};

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
    // Boxed: this payload alone is as large as the whole enum, and every
    // `Result<_, AppError>` in the crate pays for the largest variant.
    #[error("{diagnostic}")]
    Diagnostic { diagnostic: Box<ErrorDiagnostic> },
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
        Self::Diagnostic {
            diagnostic: Box::new(diagnostic),
        }
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
            // A carried diagnostic gets the same code-based domain/operation
            // inference as every other variant when it did not name its own.
            Self::Diagnostic { diagnostic } => {
                let mut diagnostic = diagnostic.clone();
                if diagnostic.domain.is_none() {
                    diagnostic.domain = infer_domain(&diagnostic.code);
                }
                if diagnostic.operation.is_none() {
                    diagnostic.operation = infer_operation(&diagnostic.code);
                }
                *diagnostic
            }
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

#[cfg(test)]
mod tests {
    use super::{
        AppError, CommandSuggestion, console_diagnostic_from_invalid_argument,
        render_console_diagnostic,
    };
    use ah_plugin_api::TextFormatter;

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
