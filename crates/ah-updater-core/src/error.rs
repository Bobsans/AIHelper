use std::fmt;

use ah_release_manifest::ManifestError;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdaterErrorCode {
    Argument,
    UnsupportedPlatform,
    Network,
    ReleaseContract,
    Trust,
    Compatibility,
    Candidate,
    Installation,
    Lock,
    Blocker,
    Transaction,
    Activation,
    Rollback,
    Recovery,
}

impl UpdaterErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Argument => "argument",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::Network => "network",
            Self::ReleaseContract => "release_contract",
            Self::Trust => "trust",
            Self::Compatibility => "compatibility",
            Self::Candidate => "candidate",
            Self::Installation => "installation",
            Self::Lock => "lock",
            Self::Blocker => "blocker",
            Self::Transaction => "transaction",
            Self::Activation => "activation",
            Self::Rollback => "rollback",
            Self::Recovery => "recovery",
        }
    }
}

impl fmt::Display for UpdaterErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {detail}")]
pub struct UpdaterError {
    code: UpdaterErrorCode,
    detail: String,
}

impl UpdaterError {
    pub fn new(code: UpdaterErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn code(&self) -> UpdaterErrorCode {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub(crate) fn from_manifest(error: ManifestError) -> Self {
        let (code, detail) = match error {
            ManifestError::UnknownKey { .. } => (
                UpdaterErrorCode::Trust,
                "release manifest uses an untrusted signing key".to_owned(),
            ),
            ManifestError::InvalidTrustRegistry { .. } => (
                UpdaterErrorCode::Trust,
                "release trust registry is invalid".to_owned(),
            ),
            ManifestError::UnsupportedSignatureAlgorithm { .. }
            | ManifestError::MalformedSignature { .. }
            | ManifestError::InvalidSignature => (
                UpdaterErrorCode::Trust,
                "release manifest signature verification failed".to_owned(),
            ),
            ManifestError::InputTooLarge { actual, maximum } => (
                UpdaterErrorCode::ReleaseContract,
                format!("release manifest is {actual} bytes; maximum is {maximum}"),
            ),
            ManifestError::UnsupportedSchema { found } => (
                UpdaterErrorCode::ReleaseContract,
                format!("release manifest schema version {found} is unsupported"),
            ),
            ManifestError::InvalidField { field, .. } => (
                UpdaterErrorCode::ReleaseContract,
                format!("release manifest field '{field}' is invalid"),
            ),
            ManifestError::MalformedJson { .. } | ManifestError::NonCanonicalEncoding => (
                UpdaterErrorCode::ReleaseContract,
                "release manifest encoding is invalid".to_owned(),
            ),
        };
        Self::new(code, detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_have_stable_snake_case_names() {
        let cases = [
            (UpdaterErrorCode::Argument, "argument"),
            (
                UpdaterErrorCode::UnsupportedPlatform,
                "unsupported_platform",
            ),
            (UpdaterErrorCode::Network, "network"),
            (UpdaterErrorCode::ReleaseContract, "release_contract"),
            (UpdaterErrorCode::Trust, "trust"),
            (UpdaterErrorCode::Compatibility, "compatibility"),
            (UpdaterErrorCode::Candidate, "candidate"),
            (UpdaterErrorCode::Installation, "installation"),
            (UpdaterErrorCode::Lock, "lock"),
            (UpdaterErrorCode::Blocker, "blocker"),
            (UpdaterErrorCode::Transaction, "transaction"),
            (UpdaterErrorCode::Activation, "activation"),
            (UpdaterErrorCode::Rollback, "rollback"),
            (UpdaterErrorCode::Recovery, "recovery"),
        ];

        for (code, expected) in cases {
            assert_eq!(code.as_str(), expected);
            assert_eq!(
                serde_json::to_string(&code).unwrap(),
                format!("\"{expected}\"")
            );
        }
    }
}
