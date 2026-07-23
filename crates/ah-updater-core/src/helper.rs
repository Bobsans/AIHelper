use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{UpdaterError, UpdaterErrorCode};

pub const UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION: u32 = 1;
pub const UPDATE_HELPER_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateHelperSelfCheckV1 {
    pub schema_version: u32,
    pub protocol_version: u32,
    pub helper_version: String,
    pub target: String,
    pub architecture: String,
}

impl UpdateHelperSelfCheckV1 {
    pub fn new(
        helper_version: impl Into<String>,
        target: impl Into<String>,
        architecture: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION,
            protocol_version: UPDATE_HELPER_PROTOCOL_VERSION,
            helper_version: helper_version.into(),
            target: target.into(),
            architecture: architecture.into(),
        }
    }

    pub fn validate(
        &self,
        expected_version: &str,
        expected_target: &str,
        expected_architecture: &str,
    ) -> Result<(), UpdaterError> {
        if self.schema_version != UPDATE_HELPER_SELF_CHECK_SCHEMA_VERSION {
            return Err(candidate(
                "update helper self-check schema version is unsupported",
            ));
        }
        if self.protocol_version != UPDATE_HELPER_PROTOCOL_VERSION {
            return Err(candidate("update helper protocol version is unsupported"));
        }
        let parsed_version = Version::parse(&self.helper_version)
            .map_err(|_| candidate("update helper version is not canonical SemVer"))?;
        if parsed_version.to_string() != self.helper_version {
            return Err(candidate("update helper version is not canonical SemVer"));
        }
        if self.helper_version != expected_version {
            return Err(candidate(
                "update helper version does not match the signed manifest",
            ));
        }
        if self.target != expected_target || self.architecture != expected_architecture {
            return Err(candidate(
                "update helper target does not match the signed manifest",
            ));
        }
        Ok(())
    }
}

fn candidate(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Candidate, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_exact_protocol_version_and_release_identity() {
        let response = UpdateHelperSelfCheckV1::new("1.2.0", "x86_64-pc-windows-msvc", "x86_64");

        response
            .validate("1.2.0", "x86_64-pc-windows-msvc", "x86_64")
            .unwrap();

        for invalid in [
            UpdateHelperSelfCheckV1 {
                schema_version: 2,
                ..response.clone()
            },
            UpdateHelperSelfCheckV1 {
                protocol_version: 2,
                ..response.clone()
            },
            UpdateHelperSelfCheckV1 {
                helper_version: "1.02.0".to_owned(),
                ..response.clone()
            },
            UpdateHelperSelfCheckV1 {
                target: "x86_64-unknown-linux-gnu".to_owned(),
                ..response.clone()
            },
        ] {
            assert_eq!(
                invalid
                    .validate("1.2.0", "x86_64-pc-windows-msvc", "x86_64")
                    .unwrap_err()
                    .code(),
                UpdaterErrorCode::Candidate
            );
        }
    }

    #[test]
    fn json_contract_is_strict_and_round_trips() {
        let response = UpdateHelperSelfCheckV1::new("1.2.0", "x86_64-pc-windows-msvc", "x86_64");
        let bytes = serde_json::to_vec(&response).unwrap();
        assert_eq!(
            serde_json::from_slice::<UpdateHelperSelfCheckV1>(&bytes).unwrap(),
            response
        );
        assert!(
            serde_json::from_str::<UpdateHelperSelfCheckV1>(
                r#"{"schema_version":1,"protocol_version":1,"helper_version":"1.2.0","target":"x86_64-pc-windows-msvc","architecture":"x86_64","extra":true}"#
            )
            .is_err()
        );
    }
}
