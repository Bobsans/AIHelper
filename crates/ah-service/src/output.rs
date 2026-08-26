use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::model::{DriftEntry, LifecycleOperation, RuntimeStatus, SCHEMA_VERSION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationStatus {
    NotInstalled,
    Installed,
    ConfigurationDrift,
    SchedulerError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationSection {
    pub status: RegistrationStatus,
    pub service_id: Option<Uuid>,
    pub configuration_id: Option<Uuid>,
    pub task_path: String,
    pub definition_path: Option<String>,
    pub diagnostic_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerState {
    NotInstalled,
    Unknown,
    Disabled,
    Queued,
    Ready,
    Running,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerSection {
    pub state: SchedulerState,
    pub last_result: Option<i32>,
    pub last_result_hex: Option<String>,
    pub last_run_at: Option<String>,
    pub diagnostic_code: Option<String>,
    pub hresult: Option<i32>,
    pub hresult_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    pub status: RuntimeStatus,
    pub service_id: Option<Uuid>,
    pub configuration_id: Option<Uuid>,
    pub version: Option<String>,
    pub instance_id: Option<Uuid>,
    pub pid: Option<u32>,
    pub endpoint: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub diagnostic_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    NotChecked,
    NotReady,
    Ready,
    IdentityMismatch,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessSection {
    pub status: ReadinessStatus,
    pub http_status: Option<u16>,
    pub version: Option<String>,
    pub instance_id: Option<Uuid>,
    pub pid: Option<u32>,
    pub diagnostic_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleOperationSection {
    pub operation_id: Uuid,
    pub operation: LifecycleOperation,
    pub pid: u32,
    pub service_id: Option<Uuid>,
    pub started_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatus {
    Idle,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSection {
    pub status: LifecycleStatus,
    pub operation: Option<LifecycleOperationSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusOutput {
    pub command: String,
    pub schema_version: u32,
    pub registration: RegistrationSection,
    pub scheduler: SchedulerSection,
    pub runtime: RuntimeSection,
    pub readiness: ReadinessSection,
    pub lifecycle: LifecycleSection,
    pub drift: Vec<DriftEntry>,
}

impl StatusOutput {
    pub fn not_installed(task_path: String) -> Self {
        Self {
            command: "mcp.service.status".to_owned(),
            schema_version: SCHEMA_VERSION,
            registration: RegistrationSection {
                status: RegistrationStatus::NotInstalled,
                service_id: None,
                configuration_id: None,
                task_path,
                definition_path: None,
                diagnostic_code: None,
            },
            scheduler: SchedulerSection {
                state: SchedulerState::NotInstalled,
                last_result: None,
                last_result_hex: None,
                last_run_at: None,
                diagnostic_code: None,
                hresult: None,
                hresult_hex: None,
            },
            runtime: RuntimeSection {
                status: RuntimeStatus::Stopped,
                service_id: None,
                configuration_id: None,
                version: None,
                instance_id: None,
                pid: None,
                endpoint: None,
                started_at: None,
                updated_at: None,
                diagnostic_code: None,
            },
            readiness: ReadinessSection {
                status: ReadinessStatus::NotChecked,
                http_status: None,
                version: None,
                instance_id: None,
                pid: None,
                diagnostic_code: None,
            },
            lifecycle: LifecycleSection {
                status: LifecycleStatus::Idle,
                operation: None,
            },
            drift: Vec::new(),
        }
    }

    pub fn sort_drift(&mut self) {
        self.drift.sort_by(|left, right| {
            left.field
                .cmp(&right.field)
                .then_with(|| drift_kind_key(&left.kind).cmp(&drift_kind_key(&right.kind)))
        });
    }
}

fn drift_kind_key(kind: &super::model::DriftKind) -> u8 {
    match kind {
        super::model::DriftKind::Missing => 0,
        super::model::DriftKind::Unexpected => 1,
        super::model::DriftKind::Mismatch => 2,
        super::model::DriftKind::Invalid => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::{super::model::UninstallOutput, *};

    #[test]
    fn not_installed_schema_keeps_all_nullable_fields() {
        let value = serde_json::to_value(StatusOutput::not_installed(r"\task".to_owned())).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 8);
        assert!(value["registration"]["service_id"].is_null());
        assert!(value["scheduler"]["last_result"].is_null());
        assert!(value["runtime"]["pid"].is_null());
        assert!(value["readiness"]["http_status"].is_null());
        assert!(value["lifecycle"]["operation"].is_null());
    }

    #[test]
    fn drift_sort_is_deterministic_by_field_then_kind() {
        let mut value = StatusOutput::not_installed(r"\task".to_owned());
        value.drift = vec![
            DriftEntry::mismatch("z", "a", "b"),
            DriftEntry::missing("a", None),
            DriftEntry::mismatch("a", "a", "b"),
        ];
        value.sort_drift();
        assert_eq!(value.drift[0].field, "a");
        assert_eq!(value.drift[0].kind, super::super::model::DriftKind::Missing);
        assert_eq!(value.drift[2].field, "z");
    }

    #[test]
    fn already_uninstalled_schema_keeps_nullable_identity_fields() {
        let value = serde_json::to_value(UninstallOutput {
            command: "mcp.service.uninstall".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed: false,
            action: "already_uninstalled".to_owned(),
            service_id: None,
            configuration_id: None,
            task_path: r"\task".to_owned(),
            endpoint: None,
            registration: "not_installed".to_owned(),
            runtime: RuntimeStatus::Stopped,
        })
        .unwrap();
        assert_eq!(value.as_object().unwrap().len(), 10);
        assert!(value["service_id"].is_null());
        assert!(value["configuration_id"].is_null());
        assert!(value["endpoint"].is_null());
    }
}
