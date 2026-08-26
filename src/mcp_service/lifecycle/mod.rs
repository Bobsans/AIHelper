//! The managed MCP service's lifecycle: install, start, stop, restart,
//! uninstall, and the status read that has to answer when those disagree.
//!
//! Every mutation goes through `with_operation`, which takes the lifecycle
//! lease, records the operation, and releases it - so this file owns the lock,
//! the shared context lookups (`require_*`) and the public entry points, and
//! one submodule per operation owns the steps:
//!
//! | Module      | Owns                                                        |
//! |-------------|-------------------------------------------------------------|
//! | `install`   | writing the definition and registering the scheduled task    |
//! | `start`     | starting the server and waiting for it to report ready       |
//! | `stop`      | stopping it, restarting it, and the orphan case              |
//! | `uninstall` | proving it inactive, then deleting only what we own          |
//! | `status`    | observing all three layers and reducing them to one state    |
//!
//! The operations used to be a single 1620-line `impl` block, which is why
//! reading one of them meant scrolling past the other four.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::{config::ConfigContext, error::AppError, output::Emitter};

use super::{
    command::{InstallOptions, ServiceCommand},
    lock::FileLease,
    model::{
        CurrentPointer, DriftEntry, DriftKind, ExitKind, LifecycleOperation, LifecycleState,
        LifecycleStateKind, MutationOutput, RuntimePhase, RuntimeState, RuntimeStatus,
        SCHEMA_VERSION, ServerDefinition, ServiceDefinition, ServiceEndpoint, TASK_SPEC_VERSION,
        TaskMarker, UninstallOutput, now_timestamp,
    },
    output::{
        LifecycleOperationSection, LifecycleStatus, ReadinessSection, ReadinessStatus,
        RegistrationStatus, SchedulerSection, SchedulerState, StatusOutput, emit_mutation,
        emit_status, emit_uninstall,
    },
    paths::{
        ServicePaths, current_executable_path, current_user_sid, normalize_absolute_path,
        paths_equal, require_managed_service_executable, task_path,
    },
    readiness::{HttpReadinessProbe, RuntimeControl, ShutdownReceipt},
    scheduler::{
        DesiredTaskSpec, ExpectedTaskOwnership, ObservedTask, SchedulerAdapter, SchedulerInstance,
        SchedulerStopTarget, TaskObservation, has_canonical_restart_policy, semantic_drift,
    },
    store::{Document, ServiceStore},
};

mod install;
mod start;
mod status;
mod stop;
mod uninstall;

use status::{
    SchedulerRuntimeEvidence, configuration_matches, reduce_runtime, require_no_drift,
    require_ready_instance_id,
};
// Reached only from `tests`, which globs this module rather than its children.
#[cfg(test)]
use status::scheduler_error_values;

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

const START_TIMEOUT: Duration = Duration::from_secs(15);

const START_POLL_INTERVAL: Duration = Duration::from_millis(100);

const STOP_TIMEOUT: Duration = Duration::from_secs(15);

const STOP_GRACE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn execute(command: ServiceCommand) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        if matches!(&command, ServiceCommand::Install(_)) {
            require_managed_service_executable(&current_executable_path()?)?;
        }
        let paths = ServicePaths::discover()?;
        let scheduler = super::windows_scheduler::WindowsTaskScheduler;
        let readiness = HttpReadinessProbe::new().map_err(|error| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("failed to create readiness client: {error}"),
            )
        })?;
        let service = LifecycleService::new(paths, scheduler, readiness);
        match command {
            ServiceCommand::Install(options) => {
                let output = service.install(&options)?;
                emit_mutation(&output, &mut Emitter::stdio(&options.options))
            }
            ServiceCommand::Start { options } => {
                let output = service.start()?;
                emit_mutation(&output, &mut Emitter::stdio(&options))
            }
            ServiceCommand::Stop { options } => {
                let output = service.stop()?;
                emit_mutation(&output, &mut Emitter::stdio(&options))
            }
            ServiceCommand::Restart { options } => {
                let output = service.restart()?;
                emit_mutation(&output, &mut Emitter::stdio(&options))
            }
            ServiceCommand::Status { options } => {
                let output = service.status();
                emit_status(&output, &mut Emitter::stdio(&options))
            }
            ServiceCommand::Uninstall { options } => {
                let output = service.uninstall()?;
                emit_uninstall(&output, &mut Emitter::stdio(&options))
            }
        }
    }

    #[cfg(not(windows))]
    {
        let _ = command;
        Err(AppError::external(
            "MCP_SERVICE_UNSUPPORTED_PLATFORM",
            "managed MCP service lifecycle is supported only on Windows",
        ))
    }
}

/// Non-printing lifecycle access for callers that render their own output,
/// such as `ah ai install --transport managed`.
#[cfg(windows)]
pub(crate) fn snapshot_status() -> Result<StatusOutput, AppError> {
    Ok(update_service()?.status())
}

#[cfg(windows)]
pub(crate) fn install_quietly(options: &InstallOptions) -> Result<MutationOutput, AppError> {
    require_managed_service_executable(&current_executable_path()?)?;
    update_service()?.install(options)
}

#[cfg(windows)]
pub(crate) fn start_quietly() -> Result<MutationOutput, AppError> {
    update_service()?.start()
}

#[cfg(windows)]
pub(crate) fn stop_for_update_while_locked() -> Result<bool, AppError> {
    let service = update_service()?;
    Ok(service.stop_locked(StopPolicy::AllowExactOrphan)?.changed)
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagedMcpUpdateState {
    pub(crate) was_running: bool,
    pub(crate) previous_instance_id: Option<Uuid>,
}

#[cfg(windows)]
pub(crate) fn capture_for_update_while_locked() -> Result<ManagedMcpUpdateState, AppError> {
    let status = update_service()?.status();
    let was_running = status.readiness.status == ReadinessStatus::Ready
        || matches!(
            status.scheduler.state,
            SchedulerState::Running | SchedulerState::Queued
        )
        || matches!(
            status.runtime.status,
            RuntimeStatus::Starting | RuntimeStatus::RunningNotReady | RuntimeStatus::Ready
        );
    let previous_instance_id = status
        .readiness
        .instance_id
        .or(status.runtime.instance_id)
        .filter(|_| was_running);
    Ok(ManagedMcpUpdateState {
        was_running,
        previous_instance_id,
    })
}

#[cfg(windows)]
pub(crate) fn restore_for_update_while_locked(
    state: ManagedMcpUpdateState,
) -> Result<(), AppError> {
    if !state.was_running {
        return Ok(());
    }
    let service = update_service()?;
    service.start_locked()?;
    let status = service.status();
    if status.readiness.status != ReadinessStatus::Ready
        || status.readiness.instance_id == state.previous_instance_id
    {
        return Err(AppError::external(
            "MCP_SERVICE_RESTART_FAILED",
            "managed MCP did not return with a new ready instance identity",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn update_service() -> Result<
    LifecycleService<super::windows_scheduler::WindowsTaskScheduler, HttpReadinessProbe>,
    AppError,
> {
    let paths = ServicePaths::discover()?;
    let readiness = HttpReadinessProbe::new().map_err(|error| {
        AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!("failed to create readiness client: {error}"),
        )
    })?;
    Ok(LifecycleService::new(
        paths,
        super::windows_scheduler::WindowsTaskScheduler,
        readiness,
    ))
}

pub struct LifecycleService<S, R> {
    store: ServiceStore,
    scheduler: S,
    readiness: R,
    start_timeout: Duration,
    stop_timeout: Duration,
    stop_grace_timeout: Duration,
    poll_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationSuccess {
    PersistCompleted,
    RemoveLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopPolicy {
    RequireRegistration,
    AllowExactOrphan,
}

struct InstalledContext {
    current: CurrentPointer,
    definition: ServiceDefinition,
    observed: ObservedTask,
    desired: DesiredTaskSpec,
}

struct StopResult {
    ownership: Option<ExpectedTaskOwnership>,
    context: Option<InstalledContext>,
    changed: bool,
    action: String,
    old_instance_id: Option<Uuid>,
    proof_guard: FileLease,
}

struct StartResult {
    changed: bool,
    action: String,
    runtime: RuntimeStatus,
    instance_id: Uuid,
}

#[cfg(test)]
mod tests;

impl<S: SchedulerAdapter, R: RuntimeControl> LifecycleService<S, R> {
    pub fn new(paths: ServicePaths, scheduler: S, readiness: R) -> Self {
        Self {
            store: ServiceStore::new(paths),
            scheduler,
            readiness,
            start_timeout: START_TIMEOUT,
            stop_timeout: STOP_TIMEOUT,
            stop_grace_timeout: STOP_GRACE_TIMEOUT,
            poll_interval: START_POLL_INTERVAL,
        }
    }

    pub fn install(&self, options: &InstallOptions) -> Result<MutationOutput, AppError> {
        self.with_operation(LifecycleOperation::Install, None, || {
            self.install_locked(options)
        })
    }

    pub fn start(&self) -> Result<MutationOutput, AppError> {
        self.with_operation(LifecycleOperation::Start, None, || self.start_locked())
    }

    pub fn stop(&self) -> Result<MutationOutput, AppError> {
        self.with_operation(LifecycleOperation::Stop, None, || self.stop_locked_output())
    }

    pub fn restart(&self) -> Result<MutationOutput, AppError> {
        self.with_operation(LifecycleOperation::Restart, None, || self.restart_locked())
    }

    pub fn uninstall(&self) -> Result<UninstallOutput, AppError> {
        self.with_operation_policy(
            LifecycleOperation::Uninstall,
            None,
            OperationSuccess::RemoveLifecycle,
            || self.uninstall_locked(),
        )
    }

    pub fn status(&self) -> StatusOutput {
        self.status_snapshot()
    }

    fn require_installed_context(&self) -> Result<InstalledContext, AppError> {
        let current = self.require_current()?;
        let definition = self.require_pointer_definition(&current)?;
        if definition.user_sid != current_user_sid()? {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP definition belongs to a different Windows user",
            ));
        }
        if current.task_path != task_path(&definition.user_sid) {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "current pointer task path does not match the service user SID",
            ));
        }
        let observation = self.scheduler.inspect(&current.task_path)?;
        let TaskObservation::Owned(observed) = observation else {
            return Err(match observation {
                TaskObservation::Missing => AppError::external(
                    "MCP_SERVICE_NOT_INSTALLED",
                    "managed MCP Task Scheduler registration is missing",
                ),
                TaskObservation::Foreign { .. } => AppError::external(
                    "MCP_SERVICE_TASK_CONFLICT",
                    "managed MCP task path is occupied by a foreign task",
                ),
                TaskObservation::Owned(_) => unreachable!(),
            });
        };
        if observed.spec.marker.service_id != definition.service_id
            || observed.spec.marker.configuration_id != definition.configuration_id
            || !paths_equal(
                &observed.spec.marker.definition_path,
                &current.definition_path,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_CONFIGURATION_DRIFT",
                "registered task does not reference the current definition",
            ));
        }
        let desired = DesiredTaskSpec::canonical(
            current.task_path.clone(),
            definition.user_sid.clone(),
            observed.spec.marker.clone(),
            definition.executable_path.clone(),
            definition.working_directory.clone(),
        );
        Ok(InstalledContext {
            current,
            definition,
            observed: observed.as_ref().clone(),
            desired,
        })
    }

    fn require_current(&self) -> Result<CurrentPointer, AppError> {
        self.store.read_current().valid()?.ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_NOT_INSTALLED",
                "managed MCP service is not installed",
            )
        })
    }

    fn require_pointer_definition(
        &self,
        pointer: &CurrentPointer,
    ) -> Result<ServiceDefinition, AppError> {
        let expected_path = self.store.paths().definition(pointer.configuration_id);
        if !paths_equal(&pointer.definition_path, &expected_path) {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "current pointer definition path is outside the managed definition store",
            ));
        }
        let definition = self
            .store
            .read_definition(&pointer.definition_path)
            .valid()?
            .ok_or_else(|| {
                AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!(
                        "managed MCP definition '{}' is missing",
                        pointer.definition_path.display()
                    ),
                )
            })?;
        if definition.service_id != pointer.service_id
            || definition.configuration_id != pointer.configuration_id
            || !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime)
            || !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "current pointer identity does not match its immutable definition",
            ));
        }
        Ok(definition)
    }

    fn require_marker_definition(
        &self,
        marker: &super::model::TaskMarker,
    ) -> Result<ServiceDefinition, AppError> {
        let expected_path = self.store.paths().definition(marker.configuration_id);
        if !paths_equal(&marker.definition_path, &expected_path) {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "registered task definition path is outside the managed definition store",
            ));
        }
        let definition = self
            .store
            .read_definition(&marker.definition_path)
            .valid()?
            .ok_or_else(|| {
                AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    "registered task references a missing immutable definition",
                )
            })?;
        if definition.service_id != marker.service_id
            || definition.configuration_id != marker.configuration_id
            || !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime)
            || !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "registered task marker identity does not match its definition",
            ));
        }
        Ok(definition)
    }

    fn observed_runtime_status(&self, definition: &ServiceDefinition) -> RuntimeStatus {
        let runtime = self.store.read_runtime().valid().ok().flatten();
        let readiness = self.readiness.inspect(definition, runtime.as_ref(), false);
        reduce_runtime(
            runtime.as_ref(),
            &readiness,
            SchedulerRuntimeEvidence::unverified(SchedulerState::Ready),
            LifecycleStatus::Busy,
            self.instance_lease_is_occupied(),
        )
    }

    fn instance_lease_is_occupied(&self) -> bool {
        match FileLease::try_acquire(&self.store.paths().instance_lock) {
            Ok(Some(lease)) => {
                drop(lease);
                false
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn cleanup_definitions(&self, pointer: &CurrentPointer) -> Result<(), AppError> {
        let mut retained = BTreeSet::from([pointer.definition_path.clone()]);
        if let Some(runtime) = self.store.read_runtime().valid()? {
            retained.insert(self.store.paths().definition(runtime.configuration_id));
        }
        for path in self.store.definition_files()? {
            if retained.iter().any(|retained| paths_equal(retained, &path)) {
                continue;
            }
            if matches!(self.store.read_definition(&path), Document::Valid(_)) {
                std::fs::remove_file(&path)
                    .map_err(|source| AppError::file_write(path.clone(), source))?;
            }
        }
        Ok(())
    }

    fn with_operation<T>(
        &self,
        operation: LifecycleOperation,
        service_id: Option<Uuid>,
        callback: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        self.with_operation_policy(
            operation,
            service_id,
            OperationSuccess::PersistCompleted,
            callback,
        )
    }

    fn with_operation_policy<T>(
        &self,
        operation: LifecycleOperation,
        service_id: Option<Uuid>,
        success: OperationSuccess,
        callback: impl FnOnce() -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let _lease =
            FileLease::acquire(&self.store.paths().lifecycle_lock, LIFECYCLE_LOCK_TIMEOUT)?;
        if let Document::UnsupportedVersion(version) = self.store.read_lifecycle() {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                format!("unsupported managed MCP lifecycle schema version {version}"),
            ));
        }
        let mut state = LifecycleState::active(operation, service_id);
        self.store.write_lifecycle(&state)?;
        match callback() {
            Ok(value) => {
                match success {
                    OperationSuccess::PersistCompleted => {
                        state.state = LifecycleStateKind::Completed;
                        state.finished_at = Some(now_timestamp());
                        self.store.write_lifecycle(&state)?;
                    }
                    OperationSuccess::RemoveLifecycle => {
                        self.store.remove_lifecycle().map_err(|error| {
                            AppError::external(
                                "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                                format!(
                                    "managed MCP lifecycle cleanup failed: {}",
                                    error.detail_message()
                                ),
                            )
                        })?;
                    }
                }
                Ok(value)
            }
            Err(error) => {
                state.state = LifecycleStateKind::Failed;
                state.diagnostic_code = Some(error.code().to_owned());
                state.finished_at = Some(now_timestamp());
                let _ = self.store.write_lifecycle(&state);
                Err(error)
            }
        }
    }
}
