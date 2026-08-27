//! The Windows projection: a [`ServiceSpec`] seen as a Task Scheduler
//! definition, and the property-level comparison that decides whether the
//! definition the Scheduler reads back is still the one we asked for.
//!
//! This is data, not COM, so it compiles and is tested on every platform -
//! `windows_scheduler` is the half that talks to the Scheduler. Keeping them
//! apart is what lets the lifecycle tests exercise the real projection and the
//! real drift comparison on a machine that has no Task Scheduler at all.
//!
//! Every one of the 27 properties below is compared deliberately. Group 06 of
//! the refactoring notes says the neutral model must keep comparing all of
//! them through the adapter rather than silently narrowing the check, which is
//! why the projection lives here rather than the spec shrinking to what a
//! generic comparison could see.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    model::{DriftEntry, TaskMarker},
    paths::{managed_service_executable_path, paths_equal},
    spec::{RestartPolicy, ServiceId, ServiceSpec, StartPolicy},
};

pub const TASK_SOURCE: &str = "AIHelper.ManagedMcp";
pub const MANAGED_RESTART_COUNT: i32 = 0;
pub const MANAGED_RESTART_INTERVAL: &str = "";

/// An unbounded execution time limit. The Task Scheduler spells "no limit" as
/// a zero-length duration rather than an absent one.
const NO_EXECUTION_TIME_LIMIT: &str = "PT0S";

pub fn task_name(owner: &str) -> String {
    format!("AIHelper Managed MCP - {owner}")
}

pub fn task_path(owner: &str) -> String {
    format!("\\{}", task_name(owner))
}

/// The identity of the managed service as Windows keeps it.
pub fn service_id(owner: String) -> ServiceId {
    let path = task_path(&owner);
    ServiceId { owner, path }
}

/// One Task Scheduler definition, as written and as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSpec {
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

impl From<&ServiceSpec> for TaskSpec {
    fn from(spec: &ServiceSpec) -> Self {
        let task_name = spec.id.path.trim_start_matches('\\').to_owned();
        Self {
            task_path: spec.id.path.clone(),
            task_name,
            user_sid: spec.id.owner.clone(),
            source: TASK_SOURCE.to_owned(),
            uri: spec.id.path.clone(),
            marker: spec.marker.clone(),
            executable_path: managed_service_executable_path(&spec.executable),
            arguments: command_line(&spec.arguments),
            working_directory: spec.working_directory.clone(),
            principal_logon_type: "interactive_token".to_owned(),
            principal_run_level: "lua".to_owned(),
            trigger_count: 1,
            trigger_type: match spec.start {
                StartPolicy::AtLogon => "logon".to_owned(),
            },
            trigger_user_sid: spec.id.owner.clone(),
            trigger_enabled: true,
            action_count: 1,
            action_type: "exec".to_owned(),
            allow_demand_start: true,
            start_when_available: true,
            multiple_instances: if spec.resource.single_instance {
                MultipleInstancesPolicy::IgnoreNew
            } else {
                MultipleInstancesPolicy::Parallel
            },
            execution_time_limit: match spec.resource.execution_time {
                None => NO_EXECUTION_TIME_LIMIT.to_owned(),
                Some(limit) => format!("PT{}S", limit.as_secs()),
            },
            disallow_start_on_batteries: false,
            stop_if_going_on_batteries: false,
            run_only_if_idle: false,
            run_only_if_network_available: false,
            restart_count: match spec.restart {
                RestartPolicy::Never => MANAGED_RESTART_COUNT,
            },
            restart_interval: match spec.restart {
                RestartPolicy::Never => MANAGED_RESTART_INTERVAL.to_owned(),
            },
            enabled: true,
        }
    }
}

/// The command line the Scheduler stores as one string.
///
/// Only arguments that could otherwise be re-split are quoted, so the string
/// is byte-identical to the one AIHelper has been registering: a path always
/// carries a separator, and no other argument in the managed command line
/// carries one.
fn command_line(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if argument.is_empty() || argument.contains([' ', '\t', '\\', '/']) {
                format!("\"{argument}\"")
            } else {
                argument.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn has_canonical_restart_policy(spec: &TaskSpec) -> bool {
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

pub fn semantic_drift(desired: &TaskSpec, observed: &TaskSpec) -> Vec<DriftEntry> {
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

fn compare_path(drift: &mut Vec<DriftEntry>, field: &str, expected: &Path, actual: &Path) {
    if !paths_equal(expected, actual) {
        drift.push(DriftEntry::mismatch(
            field,
            expected.to_string_lossy(),
            actual.to_string_lossy(),
        ));
    }
}

/// The arguments differ only in how the Scheduler spelled the definition path.
///
/// The last argument is the definition path, quoted, and paths compare by
/// identity rather than by text. Everything before it has to match exactly -
/// an appended argument leaves the quoted path no longer last, so it fails.
fn arguments_equal(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    let split = |value: &str| {
        let (head, tail) = value.rsplit_once(" \"")?;
        Some((head.to_owned(), PathBuf::from(tail.strip_suffix('"')?)))
    };
    match (split(expected), split(actual)) {
        (Some((expected_head, expected_path)), Some((actual_head, actual_path))) => {
            expected_head == actual_head && paths_equal(&expected_path, &actual_path)
        }
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

    fn platform_path(windows: &str, other: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { windows } else { other })
    }

    fn spec() -> ServiceSpec {
        let marker = TaskMarker {
            schema_version: 1,
            owner: "AIHelper".to_owned(),
            kind: "managed_mcp".to_owned(),
            service_id: Uuid::nil(),
            configuration_id: Uuid::nil(),
            definition_path: platform_path(r"C:\State\definition.json", "/state/definition.json"),
        };
        ServiceSpec::managed_mcp(
            service_id("S-1".to_owned()),
            marker,
            platform_path(r"C:\AIHelper\ah.exe", "/aihelper/ah"),
            platform_path(r"C:\Work", "/work"),
        )
    }

    fn desired() -> TaskSpec {
        TaskSpec::from(&spec())
    }

    #[test]
    fn task_identity_is_rooted_and_sid_scoped() {
        let id = service_id("S-1-5-21-1".to_owned());
        assert_eq!(id.path, r"\AIHelper Managed MCP - S-1-5-21-1");
        assert_eq!(id.owner, "S-1-5-21-1");
    }

    #[test]
    fn the_projection_keeps_the_registered_task_byte_identical() {
        let task = desired();
        assert_eq!(task.task_path, r"\AIHelper Managed MCP - S-1");
        assert_eq!(task.task_name, "AIHelper Managed MCP - S-1");
        assert_eq!(task.uri, task.task_path);
        assert_eq!(task.source, TASK_SOURCE);
        assert_eq!(task.user_sid, "S-1");
        assert_eq!(task.trigger_user_sid, "S-1");
        assert_eq!(
            task.arguments,
            format!(
                "mcp serve --transport http --managed-config \"{}\"",
                platform_path(r"C:\State\definition.json", "/state/definition.json").display()
            )
        );
        assert_eq!(task.multiple_instances, MultipleInstancesPolicy::IgnoreNew);
        assert_eq!(task.restart_count, MANAGED_RESTART_COUNT);
        assert_eq!(task.restart_interval, MANAGED_RESTART_INTERVAL);
        assert_eq!(task.restart_count, 0);
        assert!(task.restart_interval.is_empty());
        assert!(has_canonical_restart_policy(&task));
        assert_eq!(task.execution_time_limit, "PT0S");
        assert!(!task.disallow_start_on_batteries);
        assert_eq!(task.trigger_type, "logon");
        assert_eq!(task.principal_logon_type, "interactive_token");
        assert_eq!(task.principal_run_level, "lua");
        assert_eq!(task.trigger_count, 1);
        assert_eq!(task.action_count, 1);
        assert!(task.enabled);
        assert_eq!(
            task.executable_path,
            platform_path(r"C:\AIHelper\ah-mcp-service.exe", "/aihelper/ah")
        );
    }

    #[test]
    fn restart_policy_drift_is_detected_per_property() {
        let expected = desired();
        let mut count_drift = expected.clone();
        count_drift.restart_count = 1;
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
        actual.restart_count = 1;
        actual.restart_interval = "PT2M".to_owned();
        actual.arguments.push_str(" --extra");
        let drift = semantic_drift(&expected, &actual);
        assert_eq!(drift.len(), 3);
        assert_eq!(drift[0].field, "action.arguments");
        assert_eq!(drift[1].field, "settings.restart_count");
        assert_eq!(drift[2].field, "settings.restart_interval");
    }

    /// The command line differing in anything but how the Scheduler spelled the
    /// definition path is drift.
    #[test]
    fn a_changed_argument_is_drift() {
        let expected = desired();
        let mut different_flag = expected.clone();
        different_flag.arguments = different_flag
            .arguments
            .replace("--transport http", "--transport stdio");
        assert_eq!(
            semantic_drift(&expected, &different_flag)
                .into_iter()
                .map(|entry| entry.field)
                .collect::<Vec<_>>(),
            ["action.arguments"]
        );
    }
}
