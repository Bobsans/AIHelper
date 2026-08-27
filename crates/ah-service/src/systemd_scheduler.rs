//! The systemd adapter: `systemctl --user`, and the unit file it loads.
//!
//! Everything about *what* the unit says lives in [`crate::systemd_unit`];
//! this module only writes the file and asks the manager. The split mirrors
//! `windows_task` / `windows_scheduler`.
//!
//! **Experimental.** Group 06 of the refactoring notes requires it: systemd
//! brings failure modes Windows does not have, and this has nowhere near the
//! Windows acceptance coverage yet. Two are worth naming because they are the
//! caller's to fix, not ours:
//!
//! - **The user manager has to be reachable.** `systemctl --user` needs
//!   `XDG_RUNTIME_DIR` (and a running manager for the account). Where it is
//!   not, every operation here fails with the manager's own message rather
//!   than a guess.
//! - **A unit wanted by `default.target` starts when the account's manager
//!   does, which is at login.** Running it while nobody is logged in needs
//!   `loginctl enable-linger`, which needs privileges we do not have and is a
//!   different policy from the one [`crate::spec::StartPolicy::AtLogon`]
//!   states. This adapter does not enable it.

use std::{path::Path, process::Command};

use ah_error::AppError;

use crate::{
    model::DriftEntry,
    output::SchedulerState,
    scheduler::{
        ObservedService, SchedulerDeleteReceipt, SchedulerInstance, SchedulerRunReceipt,
        SchedulerStopReceipt, SchedulerStopTarget, ServiceObservation, ServiceScheduler,
    },
    spec::{ServiceId, ServiceOwnership, ServiceSpec},
    systemd_unit::{UnitSpec, semantic_drift, service_id, unit_directory, unit_path},
};

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemdUserScheduler;

/// What the manager says about a unit, as `systemctl show` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagerState {
    load_state: String,
    active_state: String,
    unit_file_state: String,
    fragment_path: String,
    needs_daemon_reload: bool,
    main_pid: u32,
    invocation_id: String,
    exec_main_status: Option<i32>,
}

impl ServiceScheduler for SystemdUserScheduler {
    type Native = UnitSpec;

    fn identity(&self) -> Result<ServiceId, AppError> {
        Ok(service_id(crate::paths::current_account()?))
    }

    fn inspect(&self, id: &ServiceId) -> Result<ServiceObservation<UnitSpec>, AppError> {
        let path = unit_path(&unit_directory()?, &id.path);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ServiceObservation::Missing);
            }
            Err(error) => {
                return Err(AppError::external(
                    "MCP_SERVICE_SCHEDULER_FAILED",
                    format!("failed to read unit '{}': {error}", path.display()),
                ));
            }
        };
        let manager = self.manager_state(&id.path)?;
        // A unit the manager loaded from somewhere else shadows ours, and what
        // it then runs is not what our file says.
        if manager.load_state == "loaded"
            && !manager.fragment_path.is_empty()
            && Path::new(&manager.fragment_path) != path
        {
            return Ok(ServiceObservation::Foreign);
        }
        // A file the manager has not read yet, or has read a stale copy of, is
        // still ours - it is drift, which the comparison reports.
        let needs_daemon_reload = manager.needs_daemon_reload || manager.load_state != "loaded";
        let Some(unit) = UnitSpec::parse(
            &text,
            &id.path,
            manager.unit_file_state == "enabled",
            needs_daemon_reload,
        ) else {
            return Ok(ServiceObservation::Foreign);
        };
        let marker = unit.marker.clone();
        Ok(ServiceObservation::Owned(Box::new(ObservedService {
            id: id.clone(),
            marker,
            state: scheduler_state(&manager),
            last_result: manager.exec_main_status,
            // systemd reports its timestamps in its own human format, and the
            // Windows adapter leaves this unset too.
            last_run_at: None,
            // systemd never restarts this unit: `Restart=no` is projected from
            // `RestartPolicy::Never`, and drift reports any other value.
            canonical_restart_policy: unit.restart == "no",
            native: unit,
        })))
    }

    fn register(&self, desired: &ServiceSpec) -> Result<ObservedService<UnitSpec>, AppError> {
        let directory = unit_directory()?;
        std::fs::create_dir_all(&directory).map_err(|error| {
            AppError::external(
                "MCP_SERVICE_SCHEDULER_FAILED",
                format!(
                    "failed to create unit directory '{}': {error}",
                    directory.display()
                ),
            )
        })?;
        let unit = UnitSpec::from(desired);
        ah_persist::atomic_write(
            &unit_path(&directory, &desired.id.path),
            unit.render()?.as_bytes(),
        )?;
        self.systemctl(&["daemon-reload"])?;
        self.systemctl(&["enable", &desired.id.path])?;
        match self.inspect(&desired.id)? {
            ServiceObservation::Owned(observed) => Ok(*observed),
            ServiceObservation::Missing => Err(AppError::external(
                "MCP_SERVICE_SCHEDULER_FAILED",
                "the registered unit disappeared during readback",
            )),
            ServiceObservation::Foreign => Err(AppError::external(
                "MCP_SERVICE_TASK_CONFLICT",
                "the registered unit does not retain AIHelper ownership markers",
            )),
        }
    }

    fn run(&self, id: &ServiceId) -> Result<SchedulerRunReceipt, AppError> {
        // `--no-block` because the lifecycle owns the wait: it polls readiness
        // rather than trusting the manager's idea of "started".
        self.systemctl(&["start", "--no-block", &id.path])?;
        Ok(SchedulerRunReceipt { submitted: true })
    }

    fn instances(&self, expected: &ServiceOwnership) -> Result<Vec<SchedulerInstance>, AppError> {
        self.require_owned(expected)?;
        let manager = self.manager_state(&expected.id.path)?;
        Ok(instance(&manager).into_iter().collect())
    }

    fn stop_instance(
        &self,
        expected: &ServiceSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        let observed = self.require_owned(&expected.ownership())?;
        let drift = semantic_drift(&UnitSpec::from(expected), &observed.native);
        if !drift.is_empty() {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                format!(
                    "unit properties changed before mutation ({} fields)",
                    drift.len()
                ),
            ));
        }
        let manager = self.manager_state(&expected.id.path)?;
        let Some(running) = instance(&manager) else {
            return Ok(SchedulerStopReceipt { stopped: false });
        };
        let matches = match target {
            SchedulerStopTarget::Running {
                instance_id,
                expected_pid,
            } => {
                running.instance_id == *instance_id
                    && running.state == SchedulerState::Running
                    && running.engine_pid == Some(*expected_pid)
            }
            SchedulerStopTarget::Queued { instance_id } => {
                running.instance_id == *instance_id
                    && running.state == SchedulerState::Queued
                    && running.engine_pid.is_none()
            }
        };
        if !matches {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                "the unit's invocation changed before stop",
            ));
        }
        self.systemctl(&["stop", "--no-block", &expected.id.path])?;
        Ok(SchedulerStopReceipt { stopped: true })
    }

    fn delete_owned(
        &self,
        expected: &ServiceOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        let path = unit_path(&unit_directory()?, &expected.id.path);
        if !path.exists() {
            return Ok(SchedulerDeleteReceipt { deleted: false });
        }
        self.require_owned(expected)?;
        // `--now` stops it as well, and a unit that is already inactive makes
        // this a no-op rather than an error.
        self.systemctl(&["disable", "--now", &expected.id.path])?;
        std::fs::remove_file(&path).map_err(|error| {
            AppError::external(
                "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                format!("failed to remove unit '{}': {error}", path.display()),
            )
        })?;
        self.systemctl(&["daemon-reload"])?;
        if path.exists() {
            return Err(AppError::external(
                "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                "the deleted unit is still present after readback",
            ));
        }
        Ok(SchedulerDeleteReceipt { deleted: true })
    }

    fn drift(
        &self,
        desired: &ServiceSpec,
        observed: &ObservedService<UnitSpec>,
    ) -> Vec<DriftEntry> {
        semantic_drift(&UnitSpec::from(desired), &observed.native)
    }
}

impl SystemdUserScheduler {
    /// The registration is still the one this installation wrote.
    fn require_owned(
        &self,
        expected: &ServiceOwnership,
    ) -> Result<ObservedService<UnitSpec>, AppError> {
        let ServiceObservation::Owned(observed) = self.inspect(&expected.id)? else {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                "unit ownership changed before mutation",
            ));
        };
        if observed.id.path != expected.id.path || observed.marker != expected.marker {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                "the unit's ownership marker changed before mutation",
            ));
        }
        Ok(*observed)
    }

    fn manager_state(&self, unit_name: &str) -> Result<ManagerState, AppError> {
        let properties = self.systemctl(&[
            "show",
            unit_name,
            "--property=LoadState,ActiveState,UnitFileState,FragmentPath,NeedDaemonReload,\
             MainPID,InvocationID,ExecMainStatus",
        ])?;
        let value = |name: &str| {
            let prefix = format!("{name}=");
            properties
                .lines()
                .find_map(|line| line.strip_prefix(&prefix))
                .unwrap_or_default()
                .trim()
                .to_owned()
        };
        Ok(ManagerState {
            load_state: value("LoadState"),
            active_state: value("ActiveState"),
            unit_file_state: value("UnitFileState"),
            fragment_path: value("FragmentPath"),
            needs_daemon_reload: value("NeedDaemonReload") == "yes",
            main_pid: value("MainPID").parse().unwrap_or(0),
            invocation_id: value("InvocationID"),
            exec_main_status: value("ExecMainStatus").parse().ok(),
        })
    }

    fn systemctl(&self, arguments: &[&str]) -> Result<String, AppError> {
        let output = Command::new("systemctl")
            .arg("--user")
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| {
                AppError::external(
                    "MCP_SERVICE_SCHEDULER_FAILED",
                    format!("failed to run `systemctl --user {}`: {error}", arguments[0]),
                )
            })?;
        if !output.status.success() {
            return Err(AppError::external(
                "MCP_SERVICE_SCHEDULER_FAILED",
                format!(
                    "`systemctl --user {}` failed: {}",
                    arguments.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The unit's current invocation, if it has one.
///
/// systemd runs at most one invocation of a non-templated unit, so where the
/// Task Scheduler can report several this reports none or one. `InvocationID`
/// is a 128-bit identity per start, which is exactly what the lifecycle needs
/// to prove it is stopping the invocation it observed.
fn instance(manager: &ManagerState) -> Option<SchedulerInstance> {
    let instance_id = uuid::Uuid::parse_str(&manager.invocation_id).ok()?;
    let state = scheduler_state(manager);
    if !matches!(state, SchedulerState::Running | SchedulerState::Queued) {
        return None;
    }
    Some(SchedulerInstance {
        instance_id,
        state,
        engine_pid: (manager.main_pid != 0).then_some(manager.main_pid),
    })
}

fn scheduler_state(manager: &ManagerState) -> SchedulerState {
    if manager.load_state != "loaded" {
        return SchedulerState::Unknown;
    }
    match manager.active_state.as_str() {
        "activating" => SchedulerState::Queued,
        "active" | "deactivating" | "reloading" => SchedulerState::Running,
        "inactive" | "failed" => {
            if manager.unit_file_state == "enabled" {
                SchedulerState::Ready
            } else {
                SchedulerState::Disabled
            }
        }
        _ => SchedulerState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(load: &str, active: &str, enabled: &str) -> ManagerState {
        ManagerState {
            load_state: load.to_owned(),
            active_state: active.to_owned(),
            unit_file_state: enabled.to_owned(),
            fragment_path: String::new(),
            needs_daemon_reload: false,
            main_pid: 4242,
            invocation_id: "550e8400e29b41d4a716446655440000".to_owned(),
            exec_main_status: Some(0),
        }
    }

    #[test]
    fn manager_states_map_onto_the_scheduler_states_the_lifecycle_reduces() {
        assert_eq!(
            scheduler_state(&manager("loaded", "activating", "enabled")),
            SchedulerState::Queued
        );
        assert_eq!(
            scheduler_state(&manager("loaded", "active", "enabled")),
            SchedulerState::Running
        );
        assert_eq!(
            scheduler_state(&manager("loaded", "inactive", "enabled")),
            SchedulerState::Ready
        );
        assert_eq!(
            scheduler_state(&manager("loaded", "inactive", "disabled")),
            SchedulerState::Disabled
        );
        assert_eq!(
            scheduler_state(&manager("not-found", "inactive", "")),
            SchedulerState::Unknown
        );
    }

    /// An inactive unit has no invocation to stop, however recently it ran.
    #[test]
    fn only_a_live_invocation_is_an_instance() {
        let running = instance(&manager("loaded", "active", "enabled")).expect("one invocation");
        assert_eq!(running.state, SchedulerState::Running);
        assert_eq!(running.engine_pid, Some(4242));
        assert_eq!(
            running.instance_id,
            uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );

        assert!(instance(&manager("loaded", "activating", "enabled")).is_some());
        assert!(instance(&manager("loaded", "inactive", "enabled")).is_none());
        assert!(instance(&manager("loaded", "failed", "enabled")).is_none());
    }

    #[test]
    fn an_invocation_without_a_main_process_reports_none_rather_than_zero() {
        let mut state = manager("loaded", "activating", "enabled");
        state.main_pid = 0;
        assert_eq!(
            instance(&state)
                .expect("activating is an instance")
                .engine_pid,
            None
        );
    }
}
