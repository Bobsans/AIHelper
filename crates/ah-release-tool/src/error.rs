use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReleaseToolError {
    #[error("cannot {operation} '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("release archive set is invalid: {detail}")]
    InvalidArchiveSet { detail: String },

    #[error("release archive '{archive}' is invalid: {detail}")]
    InvalidArchive { archive: String, detail: String },

    #[error("release archive '{archive}' entry '{entry}' is invalid: {detail}")]
    InvalidEntry {
        archive: String,
        entry: String,
        detail: String,
    },

    #[error("release signing configuration is invalid: {detail}")]
    InvalidSigningConfiguration { detail: String },

    #[error("release metadata is invalid: {detail}")]
    InvalidReleaseMetadata { detail: String },

    #[error("release manifest operation failed: {detail}")]
    Manifest { detail: String },
}

impl ReleaseToolError {
    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn archive(archive: &str, detail: impl Into<String>) -> Self {
        Self::InvalidArchive {
            archive: archive.to_owned(),
            detail: detail.into(),
        }
    }

    pub(crate) fn entry(
        archive: &str,
        entry: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::InvalidEntry {
            archive: archive.to_owned(),
            entry: entry.into(),
            detail: detail.into(),
        }
    }

    pub(crate) fn signing(detail: impl Into<String>) -> Self {
        Self::InvalidSigningConfiguration {
            detail: detail.into(),
        }
    }

    pub(crate) fn metadata(detail: impl Into<String>) -> Self {
        Self::InvalidReleaseMetadata {
            detail: detail.into(),
        }
    }

    pub(crate) fn manifest(detail: impl Into<String>) -> Self {
        Self::Manifest {
            detail: detail.into(),
        }
    }
}
