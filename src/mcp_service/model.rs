use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;

pub const SCHEMA_VERSION: u32 = 1;
pub const TASK_SPEC_VERSION: u32 = 1;
pub const DEFAULT_PORT: u16 = 8787;
pub const DEFAULT_MAX_ACTIVE: usize = 32;
pub const DEFAULT_TIMEOUT_MS: u64 = 300_000;

pub fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn validate_timestamp(value: &str) -> Result<(), AppError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|_| state_invalid(format!("timestamp '{value}' is not valid RFC 3339")))?;
    let bytes = value.as_bytes();
    let millisecond_suffix = bytes.len() >= 5
        && bytes[bytes.len() - 5] == b'.'
        && bytes[bytes.len() - 4..bytes.len() - 1]
            .iter()
            .all(u8::is_ascii_digit);
    if !value.ends_with('Z') || parsed.offset().local_minus_utc() != 0 || !millisecond_suffix {
        return Err(state_invalid(format!(
            "timestamp '{value}' must use UTC and millisecond precision"
        )));
    }
    Ok(())
}

pub fn hresult_hex(value: i32) -> String {
    format!("0x{:08X}", value as u32)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEndpoint {
    pub host: String,
    pub port: u16,
    pub mcp_url: String,
    pub readiness_url: String,
}

impl ServiceEndpoint {
    pub fn loopback(port: u16) -> Result<Self, AppError> {
        if port == 0 {
            return Err(AppError::invalid_argument("port must be greater than zero"));
        }
        Ok(Self {
            host: "127.0.0.1".to_owned(),
            port,
            mcp_url: format!("http://127.0.0.1:{port}/mcp"),
            readiness_url: format!("http://127.0.0.1:{port}/health/ready"),
        })
    }

    fn validate(&self) -> Result<(), AppError> {
        let expected = Self::loopback(self.port)?;
        if self != &expected {
            return Err(state_invalid(
                "managed MCP endpoint must use the canonical loopback URLs",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerDefinition {
    pub limit: Option<usize>,
    pub max_active: usize,
    pub default_timeout_ms: u64,
}

impl ServerDefinition {
    fn validate(&self) -> Result<(), AppError> {
        if self.limit == Some(0) {
            return Err(state_invalid("server.limit must be null or positive"));
        }
        if self.max_active == 0 {
            return Err(state_invalid("server.max_active must be positive"));
        }
        if self.default_timeout_ms == 0 {
            return Err(state_invalid("server.default_timeout_ms must be positive"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDefinition {
    pub schema_version: u32,
    pub task_spec_version: u32,
    pub service_id: Uuid,
    pub configuration_id: Uuid,
    pub user_sid: String,
    pub executable_path: PathBuf,
    pub working_directory: PathBuf,
    pub config_directory: PathBuf,
    pub runtime_state_path: PathBuf,
    pub instance_lock_path: PathBuf,
    pub expected_version: String,
    pub endpoint: ServiceEndpoint,
    pub server: ServerDefinition,
}

impl ServiceDefinition {
    pub fn validate(&self) -> Result<(), AppError> {
        require_schema(self.schema_version)?;
        if self.task_spec_version != TASK_SPEC_VERSION {
            return Err(state_invalid(format!(
                "unsupported managed MCP task spec version {}",
                self.task_spec_version
            )));
        }
        if self.user_sid.trim().is_empty() {
            return Err(state_invalid("user_sid must not be empty"));
        }
        if self.expected_version.trim().is_empty() {
            return Err(state_invalid("expected_version must not be empty"));
        }
        for (name, path) in [
            ("executable_path", &self.executable_path),
            ("working_directory", &self.working_directory),
            ("config_directory", &self.config_directory),
            ("runtime_state_path", &self.runtime_state_path),
            ("instance_lock_path", &self.instance_lock_path),
        ] {
            validate_persisted_path(name, path)?;
        }
        self.endpoint.validate()?;
        self.server.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentPointer {
    pub schema_version: u32,
    pub service_id: Uuid,
    pub configuration_id: Uuid,
    pub definition_path: PathBuf,
    pub task_path: String,
}

impl CurrentPointer {
    pub fn validate(&self) -> Result<(), AppError> {
        require_schema(self.schema_version)?;
        validate_persisted_path("definition_path", &self.definition_path)?;
        if !self.task_path.starts_with('\\') {
            return Err(state_invalid("task_path must be absolute"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskMarker {
    pub schema_version: u32,
    pub owner: String,
    pub kind: String,
    pub service_id: Uuid,
    pub configuration_id: Uuid,
    pub definition_path: PathBuf,
}

impl TaskMarker {
    pub fn from_definition(definition: &ServiceDefinition, definition_path: PathBuf) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: definition.service_id,
            configuration_id: definition.configuration_id,
            definition_path,
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        require_schema(self.schema_version)?;
        if self.owner != "AIHelper" || self.kind != "managed_mcp" {
            return Err(state_invalid("task marker is not owned by AIHelper"));
        }
        validate_persisted_path("definition_path", &self.definition_path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    Clean,
    StartupFailure,
    RuntimeFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LastExit {
    pub kind: ExitKind,
    pub exit_code: i32,
    pub diagnostic_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeState {
    pub schema_version: u32,
    pub service_id: Uuid,
    pub configuration_id: Uuid,
    pub phase: RuntimePhase,
    pub pid: u32,
    pub version: String,
    pub instance_id: Uuid,
    pub endpoint: String,
    pub started_at: String,
    pub updated_at: String,
    pub last_exit: Option<LastExit>,
}

impl RuntimeState {
    pub fn starting(definition: &ServiceDefinition, instance_id: Uuid) -> Self {
        let now = now_timestamp();
        Self {
            schema_version: SCHEMA_VERSION,
            service_id: definition.service_id,
            configuration_id: definition.configuration_id,
            phase: RuntimePhase::Starting,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            instance_id,
            endpoint: definition.endpoint.mcp_url.clone(),
            started_at: now.clone(),
            updated_at: now,
            last_exit: None,
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        require_schema(self.schema_version)?;
        if self.pid == 0 || self.version.trim().is_empty() || self.endpoint.trim().is_empty() {
            return Err(state_invalid(
                "runtime pid, version, and endpoint must be non-empty",
            ));
        }
        validate_timestamp(&self.started_at)?;
        validate_timestamp(&self.updated_at)?;
        match (self.phase, &self.last_exit) {
            (RuntimePhase::Starting | RuntimePhase::Ready | RuntimePhase::Stopping, None) => Ok(()),
            (RuntimePhase::Stopped, Some(exit))
                if exit.kind == ExitKind::Clean && exit.diagnostic_code.is_none() =>
            {
                Ok(())
            }
            (RuntimePhase::Failed, Some(exit))
                if exit.kind != ExitKind::Clean
                    && exit
                        .diagnostic_code
                        .as_deref()
                        .is_some_and(|code| !code.is_empty()) =>
            {
                Ok(())
            }
            _ => Err(state_invalid(
                "runtime phase and last_exit fields are inconsistent",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOperation {
    Install,
    Start,
    Stop,
    Restart,
    Uninstall,
    Upgrade,
    Rollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStateKind {
    Active,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleState {
    pub schema_version: u32,
    pub operation_id: Uuid,
    pub operation: LifecycleOperation,
    pub pid: u32,
    pub service_id: Option<Uuid>,
    pub started_at: String,
    pub state: LifecycleStateKind,
    pub diagnostic_code: Option<String>,
    pub finished_at: Option<String>,
}

impl LifecycleState {
    pub fn active(operation: LifecycleOperation, service_id: Option<Uuid>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            operation_id: Uuid::new_v4(),
            operation,
            pid: std::process::id(),
            service_id,
            started_at: now_timestamp(),
            state: LifecycleStateKind::Active,
            diagnostic_code: None,
            finished_at: None,
        }
    }

    pub fn validate(&self) -> Result<(), AppError> {
        require_schema(self.schema_version)?;
        if self.pid == 0 {
            return Err(state_invalid("lifecycle pid must be positive"));
        }
        validate_timestamp(&self.started_at)?;
        if let Some(finished_at) = &self.finished_at {
            validate_timestamp(finished_at)?;
        }
        match self.state {
            LifecycleStateKind::Active
                if self.diagnostic_code.is_none() && self.finished_at.is_none() =>
            {
                Ok(())
            }
            LifecycleStateKind::Completed
                if self.diagnostic_code.is_none() && self.finished_at.is_some() =>
            {
                Ok(())
            }
            LifecycleStateKind::Failed
                if self
                    .diagnostic_code
                    .as_deref()
                    .is_some_and(|code| !code.is_empty())
                    && self.finished_at.is_some() =>
            {
                Ok(())
            }
            _ => Err(state_invalid(
                "lifecycle state, diagnostic_code, and finished_at are inconsistent",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Stopped,
    Starting,
    RunningNotReady,
    Ready,
    Stopping,
    RestartBackoff,
    Failed,
    IdentityMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationOutput {
    pub command: String,
    pub schema_version: u32,
    pub changed: bool,
    pub action: String,
    pub service_id: Uuid,
    pub configuration_id: Uuid,
    pub task_path: String,
    pub endpoint: String,
    pub registration: String,
    pub runtime: RuntimeStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    Missing,
    Unexpected,
    Mismatch,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriftEntry {
    pub field: String,
    pub kind: DriftKind,
    pub expected: Option<String>,
    pub actual: Option<String>,
    pub diagnostic_code: String,
}

impl DriftEntry {
    pub fn mismatch(
        field: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            kind: DriftKind::Mismatch,
            expected: Some(expected.into()),
            actual: Some(actual.into()),
            diagnostic_code: "MCP_SERVICE_CONFIGURATION_DRIFT".to_owned(),
        }
    }

    pub fn missing(field: impl Into<String>, expected: Option<String>) -> Self {
        Self {
            field: field.into(),
            kind: DriftKind::Missing,
            expected,
            actual: None,
            diagnostic_code: "MCP_SERVICE_CONFIGURATION_DRIFT".to_owned(),
        }
    }
}

pub(crate) fn require_schema(version: u32) -> Result<(), AppError> {
    if version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(state_invalid(format!(
            "unsupported managed MCP schema version {version}"
        )))
    }
}

pub(crate) fn state_invalid(message: impl Into<String>) -> AppError {
    AppError::external("MCP_SERVICE_STATE_INVALID", message)
}

pub(crate) fn validate_uuid_json_fields(value: &serde_json::Value) -> Result<(), String> {
    match value {
        serde_json::Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "service_id" | "configuration_id" | "instance_id" | "operation_id"
                ) && !value.is_null()
                {
                    let Some(text) = value.as_str() else {
                        return Err(format!("{key} must be a UUID string or null"));
                    };
                    let uuid =
                        Uuid::parse_str(text).map_err(|_| format!("{key} must be a valid UUID"))?;
                    if uuid.to_string() != text {
                        return Err(format!(
                            "{key} must use lowercase hyphenated UUID formatting"
                        ));
                    }
                }
                validate_uuid_json_fields(value)?;
            }
            Ok(())
        }
        serde_json::Value::Array(values) => {
            for value in values {
                validate_uuid_json_fields(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_persisted_path(name: &str, path: &std::path::Path) -> Result<(), AppError> {
    if !path.is_absolute() || path.to_str().is_none() {
        return Err(state_invalid(format!(
            "{name} must be an absolute Unicode path"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition() -> ServiceDefinition {
        ServiceDefinition {
            schema_version: SCHEMA_VERSION,
            task_spec_version: TASK_SPEC_VERSION,
            service_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            configuration_id: Uuid::parse_str("22222222-2222-4222-8222-222222222222").unwrap(),
            user_sid: "S-1-5-21-1".to_owned(),
            executable_path: PathBuf::from(r"C:\AIHelper\ah.exe"),
            working_directory: PathBuf::from(r"C:\Work"),
            config_directory: PathBuf::from(r"C:\Config"),
            runtime_state_path: PathBuf::from(r"C:\State\runtime.json"),
            instance_lock_path: PathBuf::from(r"C:\State\instance.lock"),
            expected_version: "1.1.0".to_owned(),
            endpoint: ServiceEndpoint::loopback(8787).unwrap(),
            server: ServerDefinition {
                limit: None,
                max_active: 32,
                default_timeout_ms: 300_000,
            },
        }
    }

    #[test]
    fn definition_round_trips_with_exact_required_fields() {
        let expected = definition();
        expected.validate().unwrap();
        let value = serde_json::to_value(&expected).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 13);
        assert!(value["server"]["limit"].is_null());
        let actual: ServiceDefinition = serde_json::from_value(value).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn endpoint_rejects_noncanonical_loopback_contract() {
        let mut value = definition();
        value.endpoint.mcp_url = "http://localhost:8787/mcp".to_owned();
        assert_eq!(
            value.validate().unwrap_err().code(),
            "MCP_SERVICE_STATE_INVALID"
        );
    }

    #[test]
    fn runtime_requires_exit_only_for_terminal_phases() {
        let mut state = RuntimeState::starting(&definition(), Uuid::new_v4());
        state.validate().unwrap();
        state.phase = RuntimePhase::Stopped;
        assert!(state.validate().is_err());
        state.last_exit = Some(LastExit {
            kind: ExitKind::Clean,
            exit_code: 0,
            diagnostic_code: None,
        });
        state.validate().unwrap();
    }

    #[test]
    fn lifecycle_invariants_reject_failed_state_without_diagnostic() {
        let mut state = LifecycleState::active(LifecycleOperation::Install, None);
        state.state = LifecycleStateKind::Failed;
        state.finished_at = Some(now_timestamp());
        assert!(state.validate().is_err());
        state.diagnostic_code = Some("MCP_SERVICE_SCHEDULER_FAILED".to_owned());
        state.validate().unwrap();
    }

    #[test]
    fn hresults_render_as_uppercase_eight_digit_hex() {
        assert_eq!(hresult_hex(0), "0x00000000");
        assert_eq!(hresult_hex(-2_147_024_789), "0x8007006B");
    }

    #[test]
    fn timestamps_require_exact_utc_millisecond_precision() {
        validate_timestamp("2026-07-22T12:34:56.789Z").unwrap();
        assert!(validate_timestamp("2026-07-22T12:34:56Z").is_err());
        assert!(validate_timestamp("2026-07-22T12:34:56.789123Z").is_err());
        assert!(validate_timestamp("2026-07-22T15:34:56.789+03:00").is_err());
    }

    #[test]
    fn persisted_uuid_fields_require_lowercase_hyphenated_format() {
        let uppercase = serde_json::json!({
            "service_id": "11111111-1111-4111-8111-AAAAAAAAAAAA"
        });
        assert!(validate_uuid_json_fields(&uppercase).is_err());
        let lowercase = serde_json::json!({
            "service_id": "11111111-1111-4111-8111-aaaaaaaaaaaa",
            "configuration_id": null
        });
        validate_uuid_json_fields(&lowercase).unwrap();
    }
}
