use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AppError;

use super::{
    model::{DriftEntry, TaskMarker},
    output::SchedulerState,
    paths::paths_equal,
};

pub const TASK_SOURCE: &str = "AIHelper.ManagedMcp";
pub(crate) const MANAGED_RESTART_COUNT: i32 = 3;
pub(crate) const MANAGED_RESTART_INTERVAL: &str = "PT1M";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredTaskSpec {
    pub task_path: String,
    pub task_name: String,
    pub user_sid: String,
    pub source: String,
    pub uri: String,
    pub marker: TaskMarker,
    pub executable_path: PathBuf,
    pub arguments: String,
    pub working_directory: PathBuf,
    pub principal_logon_type: String,
    pub principal_run_level: String,
    pub trigger_count: i32,
    pub trigger_type: String,
    pub trigger_user_sid: String,
    pub trigger_enabled: bool,
    pub action_count: i32,
    pub action_type: String,
    pub allow_demand_start: bool,
    pub start_when_available: bool,
    pub multiple_instances: MultipleInstancesPolicy,
    pub execution_time_limit: String,
    pub disallow_start_on_batteries: bool,
    pub stop_if_going_on_batteries: bool,
    pub run_only_if_idle: bool,
    pub run_only_if_network_available: bool,
    pub restart_count: i32,
    pub restart_interval: String,
    pub enabled: bool,
}

impl DesiredTaskSpec {
    pub fn canonical(
        task_path: String,
        user_sid: String,
        marker: TaskMarker,
        executable_path: PathBuf,
        working_directory: PathBuf,
    ) -> Self {
        let task_name = task_path.trim_start_matches('\\').to_owned();
        let arguments = format!(
            "mcp serve --transport http --managed-config \"{}\"",
            marker.definition_path.display()
        );
        let trigger_user_sid = user_sid.clone();
        Self {
            task_path,
            task_name,
            user_sid,
            source: TASK_SOURCE.to_owned(),
            uri: format!("urn:aihelper:managed-mcp:v1:{}", marker.service_id),
            marker,
            executable_path,
            arguments,
            working_directory,
            principal_logon_type: "interactive_token".to_owned(),
            principal_run_level: "lua".to_owned(),
            trigger_count: 1,
            trigger_type: "logon".to_owned(),
            trigger_user_sid,
            trigger_enabled: true,
            action_count: 1,
            action_type: "exec".to_owned(),
            allow_demand_start: true,
            start_when_available: true,
            multiple_instances: MultipleInstancesPolicy::IgnoreNew,
            execution_time_limit: "PT0S".to_owned(),
            disallow_start_on_batteries: false,
            stop_if_going_on_batteries: false,
            run_only_if_idle: false,
            run_only_if_network_available: false,
            restart_count: MANAGED_RESTART_COUNT,
            restart_interval: MANAGED_RESTART_INTERVAL.to_owned(),
            enabled: true,
        }
    }
}

pub(crate) fn has_canonical_restart_policy(spec: &DesiredTaskSpec) -> bool {
    spec.restart_count == MANAGED_RESTART_COUNT && spec.restart_interval == MANAGED_RESTART_INTERVAL
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultipleInstancesPolicy {
    IgnoreNew,
    Parallel,
    Queue,
    StopExisting,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedTask {
    pub spec: DesiredTaskSpec,
    pub scheduler_state: SchedulerState,
    pub last_result: Option<i32>,
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskObservation {
    Missing,
    Owned(ObservedTask),
    Foreign {
        source: Option<String>,
        uri: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerRunReceipt {
    pub submitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedTaskOwnership {
    pub task_path: String,
    pub source: String,
    pub uri: String,
    pub marker: TaskMarker,
}

impl From<&DesiredTaskSpec> for ExpectedTaskOwnership {
    fn from(value: &DesiredTaskSpec) -> Self {
        Self {
            task_path: value.task_path.clone(),
            source: value.source.clone(),
            uri: value.uri.clone(),
            marker: value.marker.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerInstance {
    pub instance_id: uuid::Uuid,
    pub state: SchedulerState,
    pub engine_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerStopTarget {
    Running {
        instance_id: uuid::Uuid,
        expected_pid: u32,
    },
    Queued {
        instance_id: uuid::Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStopReceipt {
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerDeleteReceipt {
    pub deleted: bool,
}

pub trait SchedulerAdapter {
    fn inspect(&self, task_path: &str) -> Result<TaskObservation, AppError>;
    fn register(&self, desired: &DesiredTaskSpec) -> Result<ObservedTask, AppError>;
    fn run(&self, task_path: &str) -> Result<SchedulerRunReceipt, AppError>;
    fn instances(&self, expected: &DesiredTaskSpec) -> Result<Vec<SchedulerInstance>, AppError>;
    fn stop_instance(
        &self,
        expected: &DesiredTaskSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError>;
    fn delete_owned(
        &self,
        expected: &ExpectedTaskOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError>;
}

pub fn semantic_drift(desired: &DesiredTaskSpec, observed: &DesiredTaskSpec) -> Vec<DriftEntry> {
    let mut drift = Vec::new();
    compare(
        &mut drift,
        "task.path",
        &desired.task_path,
        &observed.task_path,
    );
    compare(
        &mut drift,
        "task.name",
        &desired.task_name,
        &observed.task_name,
    );
    compare(
        &mut drift,
        "registration.source",
        &desired.source,
        &observed.source,
    );
    compare(&mut drift, "registration.uri", &desired.uri, &observed.uri);
    compare_value(
        &mut drift,
        "registration.data.schema_version",
        desired.marker.schema_version,
        observed.marker.schema_version,
    );
    compare(
        &mut drift,
        "registration.data.owner",
        &desired.marker.owner,
        &observed.marker.owner,
    );
    compare(
        &mut drift,
        "registration.data.kind",
        &desired.marker.kind,
        &observed.marker.kind,
    );
    compare(
        &mut drift,
        "registration.data.service_id",
        &desired.marker.service_id.to_string(),
        &observed.marker.service_id.to_string(),
    );
    compare(
        &mut drift,
        "registration.data.configuration_id",
        &desired.marker.configuration_id.to_string(),
        &observed.marker.configuration_id.to_string(),
    );
    compare_path(
        &mut drift,
        "registration.data.definition_path",
        &desired.marker.definition_path,
        &observed.marker.definition_path,
    );
    compare(
        &mut drift,
        "principal.user_id",
        &desired.user_sid,
        &observed.user_sid,
    );
    compare(
        &mut drift,
        "principal.logon_type",
        &desired.principal_logon_type,
        &observed.principal_logon_type,
    );
    compare(
        &mut drift,
        "principal.run_level",
        &desired.principal_run_level,
        &observed.principal_run_level,
    );
    compare_value(
        &mut drift,
        "triggers.count",
        desired.trigger_count,
        observed.trigger_count,
    );
    compare(
        &mut drift,
        "trigger.type",
        &desired.trigger_type,
        &observed.trigger_type,
    );
    compare(
        &mut drift,
        "trigger.user_id",
        &desired.trigger_user_sid,
        &observed.trigger_user_sid,
    );
    compare_value(
        &mut drift,
        "trigger.enabled",
        desired.trigger_enabled,
        observed.trigger_enabled,
    );
    compare_value(
        &mut drift,
        "actions.count",
        desired.action_count,
        observed.action_count,
    );
    compare(
        &mut drift,
        "action.type",
        &desired.action_type,
        &observed.action_type,
    );
    compare_path(
        &mut drift,
        "action.path",
        &desired.executable_path,
        &observed.executable_path,
    );
    if !arguments_equal(&desired.arguments, &observed.arguments) {
        drift.push(DriftEntry::mismatch(
            "action.arguments",
            &desired.arguments,
            &observed.arguments,
        ));
    }
    compare_path(
        &mut drift,
        "action.working_directory",
        &desired.working_directory,
        &observed.working_directory,
    );
    compare_value(
        &mut drift,
        "settings.allow_demand_start",
        desired.allow_demand_start,
        observed.allow_demand_start,
    );
    compare_value(
        &mut drift,
        "settings.start_when_available",
        desired.start_when_available,
        observed.start_when_available,
    );
    compare_value(
        &mut drift,
        "settings.multiple_instances",
        desired.multiple_instances,
        observed.multiple_instances,
    );
    compare(
        &mut drift,
        "settings.execution_time_limit",
        &desired.execution_time_limit,
        &observed.execution_time_limit,
    );
    compare_value(
        &mut drift,
        "settings.disallow_start_on_batteries",
        desired.disallow_start_on_batteries,
        observed.disallow_start_on_batteries,
    );
    compare_value(
        &mut drift,
        "settings.stop_if_going_on_batteries",
        desired.stop_if_going_on_batteries,
        observed.stop_if_going_on_batteries,
    );
    compare_value(
        &mut drift,
        "settings.run_only_if_idle",
        desired.run_only_if_idle,
        observed.run_only_if_idle,
    );
    compare_value(
        &mut drift,
        "settings.run_only_if_network_available",
        desired.run_only_if_network_available,
        observed.run_only_if_network_available,
    );
    compare_value(
        &mut drift,
        "settings.restart_count",
        desired.restart_count,
        observed.restart_count,
    );
    compare(
        &mut drift,
        "settings.restart_interval",
        &desired.restart_interval,
        &observed.restart_interval,
    );
    compare_value(
        &mut drift,
        "settings.enabled",
        desired.enabled,
        observed.enabled,
    );
    drift.sort_by(|left, right| left.field.cmp(&right.field));
    drift
}

fn compare(drift: &mut Vec<DriftEntry>, field: &str, expected: &str, actual: &str) {
    if expected != actual {
        drift.push(DriftEntry::mismatch(field, expected, actual));
    }
}

fn compare_path(
    drift: &mut Vec<DriftEntry>,
    field: &str,
    expected: &std::path::Path,
    actual: &std::path::Path,
) {
    if !paths_equal(expected, actual) {
        drift.push(DriftEntry::mismatch(
            field,
            expected.to_string_lossy(),
            actual.to_string_lossy(),
        ));
    }
}

fn arguments_equal(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    let prefix = "mcp serve --transport http --managed-config \"";
    let extract = |value: &str| {
        value
            .strip_prefix(prefix)
            .and_then(|value| value.strip_suffix('"'))
            .map(PathBuf::from)
    };
    match (extract(expected), extract(actual)) {
        (Some(expected), Some(actual)) => paths_equal(&expected, &actual),
        _ => false,
    }
}

fn compare_value<T: std::fmt::Debug + PartialEq>(
    drift: &mut Vec<DriftEntry>,
    field: &str,
    expected: T,
    actual: T,
) {
    if expected != actual {
        drift.push(DriftEntry::mismatch(
            field,
            format!("{expected:?}").to_lowercase(),
            format!("{actual:?}").to_lowercase(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn desired() -> DesiredTaskSpec {
        let marker = TaskMarker {
            schema_version: 1,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: Uuid::nil(),
            configuration_id: Uuid::nil(),
            definition_path: if cfg!(windows) {
                PathBuf::from(r"C:\State\definition.json")
            } else {
                PathBuf::from("/state/definition.json")
            },
        };
        DesiredTaskSpec::canonical(
            r"\AIHelper Managed MCP - S-1".to_owned(),
            "S-1".to_owned(),
            marker,
            if cfg!(windows) {
                PathBuf::from(r"C:\AIHelper\ah.exe")
            } else {
                PathBuf::from("/aihelper/ah")
            },
            if cfg!(windows) {
                PathBuf::from(r"C:\Work")
            } else {
                PathBuf::from("/work")
            },
        )
    }

    #[test]
    fn canonical_spec_contains_restart_and_single_instance_policy() {
        let spec = desired();
        assert_eq!(spec.multiple_instances, MultipleInstancesPolicy::IgnoreNew);
        assert_eq!(spec.restart_count, MANAGED_RESTART_COUNT);
        assert_eq!(spec.restart_interval, MANAGED_RESTART_INTERVAL);
        assert!(has_canonical_restart_policy(&spec));
        assert_eq!(spec.execution_time_limit, "PT0S");
        assert!(!spec.disallow_start_on_batteries);
    }

    #[test]
    fn restart_policy_drift_is_detected_per_property() {
        let expected = desired();
        let mut count_drift = expected.clone();
        count_drift.restart_count = 0;
        assert_eq!(
            semantic_drift(&expected, &count_drift)
                .into_iter()
                .map(|entry| entry.field)
                .collect::<Vec<_>>(),
            ["settings.restart_count"]
        );

        let mut interval_drift = expected.clone();
        interval_drift.restart_interval = "PT2M".to_owned();
        assert_eq!(
            semantic_drift(&expected, &interval_drift)
                .into_iter()
                .map(|entry| entry.field)
                .collect::<Vec<_>>(),
            ["settings.restart_interval"]
        );
    }

    #[test]
    fn semantic_drift_is_property_level_and_sorted() {
        let expected = desired();
        let mut actual = expected.clone();
        actual.restart_count = 0;
        actual.restart_interval = "PT2M".to_owned();
        actual.arguments.push_str(" --extra");
        let drift = semantic_drift(&expected, &actual);
        assert_eq!(drift.len(), 3);
        assert_eq!(drift[0].field, "action.arguments");
        assert_eq!(drift[1].field, "settings.restart_count");
        assert_eq!(drift[2].field, "settings.restart_interval");
    }
}
