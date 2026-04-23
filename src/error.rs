use std::{io, path::PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
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
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "INVALID_ARGUMENT",
            Self::ChangeDirectory { .. } => "CWD_CHANGE_FAILED",
            Self::FileRead { .. } => "FILE_READ_FAILED",
            Self::FileWrite { .. } => "FILE_WRITE_FAILED",
            Self::FileMetadata { .. } => "FILE_METADATA_FAILED",
            Self::DirectoryRead { .. } => "DIRECTORY_READ_FAILED",
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
        eprintln!("[{}] {}", self.code(), self);
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
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
}
