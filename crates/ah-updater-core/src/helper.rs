use semver::Version;
use std::ffi::OsStr;

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

/// The flags the update handoff passes, in the order the helper reads them.
///
/// A cross-version contract: the `ah` of one release launches the helper of
/// another, and the helper validates its arguments *by position*. `ah` builds
/// the command line in two halves - the roots before the launch, the inherited
/// lease and the acknowledgement event during it - so this is the one place the
/// order is written down, and the two builders below are how both halves are
/// produced.
pub const HANDOFF_FLAGS: [&str; 6] = [
    "--installation-root",
    "--installation-state-root",
    "--transaction-root",
    "--lifecycle-lock",
    "--lifecycle-lock-handle",
    "--handoff-event",
];

/// The operation, one flag and value per entry above, and the parent pid.
pub const HANDOFF_ARGUMENT_COUNT: usize = 2 + 2 * HANDOFF_FLAGS.len();

/// The first half of the handoff command line: the operation and the three
/// roots, known before the helper is launched.
#[must_use]
pub fn handoff_paths_arguments<'a>(
    operation: &'a OsStr,
    installation_root: &'a OsStr,
    installation_state_root: &'a OsStr,
    transaction_root: &'a OsStr,
) -> [&'a OsStr; 7] {
    [
        operation,
        OsStr::new(HANDOFF_FLAGS[0]),
        installation_root,
        OsStr::new(HANDOFF_FLAGS[1]),
        installation_state_root,
        OsStr::new(HANDOFF_FLAGS[2]),
        transaction_root,
    ]
}

/// The second half: the lease the child inherits, the event it acknowledges on,
/// and the parent it waits for. Only the launcher knows these.
#[must_use]
pub fn handoff_lease_arguments<'a>(
    lifecycle_lock: &'a OsStr,
    lifecycle_lock_handle: &'a OsStr,
    handoff_event: &'a OsStr,
    parent_pid: &'a OsStr,
) -> [&'a OsStr; 7] {
    [
        OsStr::new(HANDOFF_FLAGS[3]),
        lifecycle_lock,
        OsStr::new(HANDOFF_FLAGS[4]),
        lifecycle_lock_handle,
        OsStr::new(HANDOFF_FLAGS[5]),
        handoff_event,
        parent_pid,
    ]
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
