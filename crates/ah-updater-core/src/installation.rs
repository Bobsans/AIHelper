use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{UpdaterError, UpdaterErrorCode};

pub const INSTALLATION_IDENTITY_SCHEMA_VERSION: u32 = 1;

pub fn lifecycle_mutex_name(path: &Path) -> String {
    format!(
        "Global\\AIHelper-MCP-{}",
        path.to_string_lossy().replace(['\\', '/', ':'], "-")
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationIdentityV1 {
    pub schema_version: u32,
    pub installation_id: Uuid,
    pub executable_path: String,
}

impl InstallationIdentityV1 {
    pub fn new(
        installation_id: Uuid,
        executable_path: impl Into<String>,
    ) -> Result<Self, UpdaterError> {
        let identity = Self {
            schema_version: INSTALLATION_IDENTITY_SCHEMA_VERSION,
            installation_id,
            executable_path: executable_path.into(),
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), UpdaterError> {
        if self.schema_version != INSTALLATION_IDENTITY_SCHEMA_VERSION {
            return Err(installation(
                "installation identity schema version is unsupported",
            ));
        }
        if self.installation_id.is_nil() {
            return Err(installation("installation identity UUID is invalid"));
        }
        if self.executable_path.is_empty() || self.executable_path.contains('\0') {
            return Err(installation(
                "installation identity executable path is invalid",
            ));
        }
        Ok(())
    }
}

fn installation(detail: &'static str) -> UpdaterError {
    UpdaterError::new(UpdaterErrorCode::Installation, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_mutex_name_is_deterministic_for_windows_paths() {
        assert_eq!(
            lifecycle_mutex_name(std::path::Path::new(
                r"C:\Users\Example\AppData\Local\AIHelper\managed-mcp\lifecycle.lock",
            )),
            "Global\\AIHelper-MCP-C--Users-Example-AppData-Local-AIHelper-managed-mcp-lifecycle.lock"
        );
    }

    #[test]
    fn identity_round_trips_with_strict_canonical_fields() {
        let identity =
            InstallationIdentityV1::new(Uuid::new_v4(), r"C:\Tools\AIHelper\ah.exe").unwrap();
        let json = serde_json::to_string(&identity).unwrap();
        let decoded: InstallationIdentityV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, identity);
        decoded.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_identity_fields_and_unknown_json() {
        let invalid = InstallationIdentityV1 {
            schema_version: 2,
            installation_id: Uuid::nil(),
            executable_path: String::new(),
        };
        assert_eq!(
            invalid.validate().unwrap_err().code(),
            UpdaterErrorCode::Installation
        );
        assert!(
            serde_json::from_str::<InstallationIdentityV1>(
                r#"{"schema_version":1,"installation_id":"00000000-0000-0000-0000-000000000001","executable_path":"C:\\ah.exe","extra":true}"#
            )
            .is_err()
        );
    }
}
