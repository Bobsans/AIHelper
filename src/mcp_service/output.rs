use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::AppError, output::OutputMode};

use super::model::{
    DriftEntry, LifecycleOperation, MutationOutput, RuntimeStatus, SCHEMA_VERSION, UninstallOutput,
};

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

pub fn emit_mutation(
    value: &MutationOutput,
    mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(value)?),
        OutputMode::Text => {
            println!("command={}", value.command);
            println!("schema_version={}", value.schema_version);
            println!("changed={}", value.changed);
            println!("action={}", value.action);
            println!("service_id={}", value.service_id);
            println!("configuration_id={}", value.configuration_id);
            println!("task_path={}", value.task_path);
            println!("endpoint={}", value.endpoint);
            println!("registration={}", enum_json(&value.registration)?);
            println!("runtime={}", enum_json(&value.runtime)?);
        }
    }
    Ok(())
}

pub fn emit_uninstall(
    value: &UninstallOutput,
    mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(value)?),
        OutputMode::Text => {
            println!("command={}", value.command);
            println!("schema_version={}", value.schema_version);
            println!("changed={}", value.changed);
            println!("action={}", value.action);
            println!(
                "service_id={}",
                value
                    .service_id
                    .map(|value| value.to_string())
                    .as_deref()
                    .unwrap_or("null")
            );
            println!(
                "configuration_id={}",
                value
                    .configuration_id
                    .map(|value| value.to_string())
                    .as_deref()
                    .unwrap_or("null")
            );
            println!("task_path={}", value.task_path);
            println!("endpoint={}", value.endpoint.as_deref().unwrap_or("null"));
            println!("registration={}", value.registration);
            println!("runtime={}", enum_json(&value.runtime)?);
        }
    }
    Ok(())
}

pub fn emit_status(value: &StatusOutput, mode: OutputMode, quiet: bool) -> Result<(), AppError> {
    if quiet {
        return Ok(());
    }
    match mode {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(value)?),
        OutputMode::Text => {
            println!("command={}", value.command);
            println!("schema_version={}", value.schema_version);
            println!(
                "registration.status={}",
                enum_json(&value.registration.status)?
            );
            println!("registration.task_path={}", value.registration.task_path);
            println!("scheduler.state={}", enum_json(&value.scheduler.state)?);
            println!("runtime.status={}", enum_json(&value.runtime.status)?);
            println!("readiness.status={}", enum_json(&value.readiness.status)?);
            println!("lifecycle.status={}", enum_json(&value.lifecycle.status)?);
            println!("drift.count={}", value.drift.len());
            for drift in &value.drift {
                println!(
                    "drift={} kind={} expected={} actual={} code={}",
                    drift.field,
                    enum_json(&drift.kind)?,
                    drift.expected.as_deref().unwrap_or("null"),
                    drift.actual.as_deref().unwrap_or("null"),
                    drift.diagnostic_code
                );
            }
        }
    }
    Ok(())
}

fn enum_json<T: Serialize>(value: &T) -> Result<String, AppError> {
    Ok(serde_json::to_string(value)?.trim_matches('"').to_owned())
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
    use super::*;

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
