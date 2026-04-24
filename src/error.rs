use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{message}")]
    External { code: String, message: String },
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
        let rendered = self.rendered();
        eprintln!("error[{}]: {}", rendered.code, rendered.message);
        for context in rendered.context {
            eprintln!("  {}: {}", context.label, context.value);
        }
        if let Some(hint) = rendered.hint {
            eprintln!("hint: {hint}");
        }
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
                hint: Some("check --cwd and ensure the directory exists".to_owned()),
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
                hint: hint_for_code(self.code()),
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
                hint: None,
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
                hint: hint_for_code(self.code()),
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
                hint: hint_for_code(self.code()),
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
                hint: None,
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
                hint: None,
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
                hint: None,
            },
            Self::JsonSerialization(source) => RenderedError {
                code: self.code().to_owned(),
                message: "failed to serialize JSON output".to_owned(),
                context: vec![RenderedContext {
                    label: "reason",
                    value: source.to_string(),
                }],
                hint: None,
            },
        }
    }
}

#[derive(Debug)]
struct RenderedError {
    code: String,
    message: String,
    context: Vec<RenderedContext>,
    hint: Option<String>,
}

#[derive(Debug)]
struct RenderedContext {
    label: &'static str,
    value: String,
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
            hint: hint_for_code("PATH_NOT_FOUND"),
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
            hint: hint_for_code("PATH_INVALID_TYPE"),
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
            hint: hint_for_code("TASK_NOT_FOUND"),
        };
    }

    let code = classify_invalid_argument(&message);
    RenderedError {
        code: code.to_owned(),
        message,
        context: Vec::new(),
        hint: hint_for_code(code),
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

fn hint_for_code(code: &str) -> Option<String> {
    match code {
        "PATH_NOT_FOUND" => {
            Some("check path spelling or set --cwd to the correct workspace".to_owned())
        }
        "PATH_INVALID_TYPE" => {
            Some("provide a file or directory path expected by the command".to_owned())
        }
        "TASK_NOT_FOUND" => Some("run `ah task list` to inspect available saved tasks".to_owned()),
        "SYMLINK_TRAVERSAL_BLOCKED" => {
            Some("use --follow-symlinks if traversal is intentional".to_owned())
        }
        "REGEX_INVALID" => {
            Some("fix the regex syntax or remove --regex for plain text search".to_owned())
        }
        "DOMAIN_NOT_FOUND" => {
            Some("run `ah plugins list` to inspect available command domains".to_owned())
        }
        "DOMAIN_DISABLED" => {
            Some("run `ah plugins enable <domain>` to re-enable the domain".to_owned())
        }
        "FILE_NOT_FOUND" => {
            Some("verify path exists and current working directory is correct".to_owned())
        }
        "DIRECTORY_NOT_FOUND" => {
            Some("verify directory exists and current working directory is correct".to_owned())
        }
        _ => None,
    }
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
        hint: hint_for_code(code),
    }
}
