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
//! | `guard`     | the same service, seen as something an update holds still     |
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

use ah_config::ConfigContext;
use ah_error::AppError;

use super::{
    lock::{self, FileLease},
    model::{
        CurrentPointer, DriftEntry, DriftKind, ExitKind, LifecycleOperation, LifecycleState,
        LifecycleStateKind, MutationOutput, RuntimePhase, RuntimeState, RuntimeStatus,
        SCHEMA_VERSION, ServerDefinition, ServiceDefinition, ServiceEndpoint, TASK_SPEC_VERSION,
        TaskMarker, UninstallOutput, now_timestamp,
    },
    operation::{InstallSettings, Operation, Report},
    output::{
        LifecycleOperationSection, LifecycleStatus, ReadinessSection, ReadinessStatus,
        RegistrationStatus, SchedulerSection, SchedulerState, StatusOutput,
    },
    paths::{
        ServicePaths, current_executable_path, normalize_absolute_path, paths_equal,
        require_managed_service_executable,
    },
    readiness::{HttpReadinessProbe, RuntimeControl, ShutdownReceipt},
    scheduler::{
        ObservedService, PlatformScheduler, SchedulerInstance, SchedulerStopTarget,
        ServiceObservation, ServiceScheduler, platform_scheduler,
    },
    spec::{ServiceId, ServiceOwnership, ServiceSpec},
    store::{Document, ServiceStore},
};

mod guard;
mod install;
mod start;
mod status;
mod stop;
mod uninstall;

pub use guard::ManagedMcpGuard;

use status::{
    SchedulerRuntimeEvidence, configuration_matches, reduce_runtime, require_ready_instance_id,
};
// Reached only from `tests`, which globs this module rather than its children.
#[cfg(test)]
use status::scheduler_error_values;

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

const START_TIMEOUT: Duration = Duration::from_secs(15);

const START_POLL_INTERVAL: Duration = Duration::from_millis(100);

const STOP_TIMEOUT: Duration = Duration::from_secs(15);

const STOP_GRACE_TIMEOUT: Duration = Duration::from_secs(5);

/// Run one lifecycle operation and report what it did.
///
/// # Errors
///
/// [`AppError`] when the operation fails, or on a platform where there is no
/// managed service to operate - which arrives as a refusal from that
/// platform's scheduler rather than as a branch of this function.
pub fn run(operation: Operation) -> Result<Report, AppError> {
    if matches!(&operation, Operation::Install(_)) {
        require_managed_service_executable(&current_executable_path()?)?;
    }
    let service = platform_service()?;
    match operation {
        Operation::Install(settings) => service
            .install(&settings)
            .map(|output| Report::Mutation(Box::new(output))),
        Operation::Start => service
            .start()
            .map(|output| Report::Mutation(Box::new(output))),
        Operation::Stop => service
            .stop()
            .map(|output| Report::Mutation(Box::new(output))),
        Operation::Restart => service
            .restart()
            .map(|output| Report::Mutation(Box::new(output))),
        Operation::Status => Ok(Report::Status(Box::new(service.status()))),
        Operation::Uninstall => service
            .uninstall()
            .map(|output| Report::Uninstall(Box::new(output))),
    }
}

/// Non-printing lifecycle access for callers that render their own output,
/// such as `ah ai install --transport managed`.
pub fn snapshot_status() -> Result<StatusOutput, AppError> {
    Ok(platform_service()?.status())
}

pub fn install_quietly(settings: &InstallSettings) -> Result<MutationOutput, AppError> {
    require_managed_service_executable(&current_executable_path()?)?;
    platform_service()?.install(settings)
}

pub fn start_quietly() -> Result<MutationOutput, AppError> {
    platform_service()?.start()
}

/// The managed service as this platform holds it.
fn platform_service() -> Result<LifecycleService<PlatformScheduler, HttpReadinessProbe>, AppError> {
    let paths = ServicePaths::discover()?;
    let readiness = HttpReadinessProbe::new().map_err(|error| {
        AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!("failed to create readiness client: {error}"),
        )
    })?;
    Ok(LifecycleService::new(
        paths,
        platform_scheduler(),
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
pub(crate) enum OperationSuccess {
    PersistCompleted,
    RemoveLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopPolicy {
    RequireRegistration,
    AllowExactOrphan,
}

pub(crate) struct InstalledContext<N> {
    current: CurrentPointer,
    definition: ServiceDefinition,
    observed: ObservedService<N>,
    desired: ServiceSpec,
}

pub(crate) struct StopResult<N> {
    ownership: Option<ServiceOwnership>,
    context: Option<InstalledContext<N>>,
    changed: bool,
    action: String,
    old_instance_id: Option<Uuid>,
    proof_guard: FileLease,
}

pub(crate) struct StartResult {
    changed: bool,
    action: String,
    runtime: RuntimeStatus,
    instance_id: Uuid,
}

#[cfg(test)]
mod tests;

impl<S: ServiceScheduler, R: RuntimeControl> LifecycleService<S, R> {
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

    pub fn install(&self, options: &InstallSettings) -> Result<MutationOutput, AppError> {
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

    fn require_installed_context(&self) -> Result<InstalledContext<S::Native>, AppError> {
        let current = self.require_current()?;
        let definition = self.require_pointer_definition(&current)?;
        let identity = self.scheduler.identity()?;
        if definition.user_sid != identity.owner {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP definition belongs to a different user account",
            ));
        }
        if current.task_path != identity.path {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "current pointer task path does not match the service user SID",
            ));
        }
        let observation = self.scheduler.inspect(&identity)?;
        let ServiceObservation::Owned(observed) = observation else {
            return Err(match observation {
                ServiceObservation::Missing => AppError::external(
                    "MCP_SERVICE_NOT_INSTALLED",
                    "managed MCP Task Scheduler registration is missing",
                ),
                ServiceObservation::Foreign => AppError::external(
                    "MCP_SERVICE_TASK_CONFLICT",
                    "managed MCP task path is occupied by a foreign task",
                ),
                ServiceObservation::Owned(_) => unreachable!(),
            });
        };
        if observed.marker.service_id != definition.service_id
            || observed.marker.configuration_id != definition.configuration_id
            || !paths_equal(&observed.marker.definition_path, &current.definition_path)
        {
            return Err(AppError::external(
                "MCP_SERVICE_CONFIGURATION_DRIFT",
                "registered task does not reference the current definition",
            ));
        }
        let desired = ServiceSpec::managed_mcp(
            identity,
            observed.marker.clone(),
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
        !lock::is_free(&self.store.paths().instance_lock).unwrap_or(true)
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
        let _lease = lock::acquire(&self.store.paths().lifecycle_lock, LIFECYCLE_LOCK_TIMEOUT)?;
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
