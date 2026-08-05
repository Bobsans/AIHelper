use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::{config::ConfigContext, error::AppError};

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
    paths::{ServicePaths, current_user_sid, normalize_absolute_path, paths_equal, task_path},
    readiness::{HttpReadinessProbe, RuntimeControl, ShutdownReceipt},
    scheduler::{
        DesiredTaskSpec, ExpectedTaskOwnership, ObservedTask, SchedulerAdapter, SchedulerInstance,
        SchedulerStopTarget, TaskObservation, has_canonical_restart_policy, semantic_drift,
    },
    store::{Document, ServiceStore},
};

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const START_POLL_INTERVAL: Duration = Duration::from_millis(100);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_GRACE_TIMEOUT: Duration = Duration::from_secs(5);

pub fn execute(command: ServiceCommand) -> Result<(), AppError> {
    #[cfg(windows)]
    {
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
                emit_mutation(&output, options.options.output, options.options.quiet)
            }
            ServiceCommand::Start { options } => {
                let output = service.start()?;
                emit_mutation(&output, options.output, options.quiet)
            }
            ServiceCommand::Stop { options } => {
                let output = service.stop()?;
                emit_mutation(&output, options.output, options.quiet)
            }
            ServiceCommand::Restart { options } => {
                let output = service.restart()?;
                emit_mutation(&output, options.output, options.quiet)
            }
            ServiceCommand::Status { options } => {
                let output = service.status();
                emit_status(&output, options.output, options.quiet)
            }
            ServiceCommand::Uninstall { options } => {
                let output = service.uninstall()?;
                emit_uninstall(&output, options.output, options.quiet)
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

#[cfg(windows)]
pub(crate) fn stop_for_update_while_locked() -> Result<bool, AppError> {
    let paths = ServicePaths::discover()?;
    let readiness = HttpReadinessProbe::new().map_err(|error| {
        AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!("failed to create readiness client: {error}"),
        )
    })?;
    let service = LifecycleService::new(
        paths,
        super::windows_scheduler::WindowsTaskScheduler,
        readiness,
    );
    Ok(service.stop_locked(StopPolicy::AllowExactOrphan)?.changed)
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

    fn install_locked(&self, options: &InstallOptions) -> Result<MutationOutput, AppError> {
        if options.max_active == 0 || options.default_timeout_ms == 0 {
            return Err(AppError::invalid_argument(
                "--max-active and --default-timeout-ms must be positive",
            ));
        }
        if options.options.limit == Some(0) {
            return Err(AppError::invalid_argument("--limit must be positive"));
        }
        self.store.ensure_directories()?;
        let cwd = normalize_absolute_path(
            &std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))?,
            None,
        )?;
        let executable = normalize_absolute_path(
            &std::env::current_exe().map_err(|error| {
                AppError::external(
                    "MCP_SERVICE_PATH_INVALID",
                    format!("failed to resolve current executable: {error}"),
                )
            })?,
            None,
        )?;
        let config = ConfigContext::load()?;
        let config_dir = normalize_absolute_path(&config.paths().config_dir, Some(&cwd))?;
        let user_sid = current_user_sid()?;
        let expected_task_path = task_path(&user_sid);
        let current_document = self.store.read_current();
        let current = match current_document {
            Document::Missing => None,
            Document::Valid(current) => Some(current),
            Document::Invalid(message) => {
                return Err(AppError::external("MCP_SERVICE_STATE_INVALID", message));
            }
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP schema version {version}"),
                ));
            }
        };
        // Reject corrupt or newer runtime documents before changing the task or
        // current pointer. A valid stale runtime remains diagnostic input.
        let _ = self.store.read_runtime().valid()?;
        let observation = self.scheduler.inspect(&expected_task_path)?;
        let was_installed = !matches!(observation, TaskObservation::Missing);
        if matches!(observation, TaskObservation::Foreign { .. }) {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CONFLICT",
                format!("Task Scheduler path '{expected_task_path}' is not owned by AIHelper"),
            ));
        }

        let task_definition = match &observation {
            TaskObservation::Owned(observed) => {
                Some(self.require_marker_definition(&observed.spec.marker)?)
            }
            TaskObservation::Missing | TaskObservation::Foreign { .. } => None,
        };
        if let (Some(task), Some(current)) = (&task_definition, &current)
            && task.service_id != current.service_id
        {
            return Err(AppError::external(
                "MCP_SERVICE_INSTALLATION_CONFLICT",
                "registered task and current pointer identify different services",
            ));
        }
        let pointer_definition = if task_definition.is_none() {
            current
                .as_ref()
                .map(|pointer| self.require_pointer_definition(pointer))
                .transpose()?
        } else {
            None
        };
        let existing = task_definition.as_ref().or(pointer_definition.as_ref());
        if let Some(existing) = existing
            && !paths_equal(&existing.executable_path, &executable)
        {
            return Err(AppError::external(
                "MCP_SERVICE_INSTALLATION_CONFLICT",
                format!(
                    "managed MCP service belongs to executable '{}'",
                    existing.executable_path.display()
                ),
            ));
        }

        let service_id = existing
            .map(|definition| definition.service_id)
            .unwrap_or_else(Uuid::new_v4);
        let endpoint = ServiceEndpoint::loopback(options.port)?;
        let server = ServerDefinition {
            limit: options.options.limit,
            max_active: options.max_active,
            default_timeout_ms: options.default_timeout_ms,
        };
        let configuration_id = existing
            .filter(|existing| {
                configuration_matches(
                    existing,
                    &user_sid,
                    &executable,
                    &cwd,
                    &config_dir,
                    &self.store.paths().runtime,
                    &self.store.paths().instance_lock,
                    &endpoint,
                    &server,
                )
            })
            .map(|existing| existing.configuration_id)
            .unwrap_or_else(Uuid::new_v4);
        let definition = ServiceDefinition {
            schema_version: SCHEMA_VERSION,
            task_spec_version: TASK_SPEC_VERSION,
            service_id,
            configuration_id,
            user_sid: user_sid.clone(),
            executable_path: executable.clone(),
            working_directory: cwd.clone(),
            config_directory: config_dir,
            runtime_state_path: self.store.paths().runtime.clone(),
            instance_lock_path: self.store.paths().instance_lock.clone(),
            expected_version: env!("CARGO_PKG_VERSION").to_owned(),
            endpoint,
            server,
        };
        definition.validate()?;
        let definition_path = self.store.paths().definition(configuration_id);
        let definition_created = self
            .store
            .write_immutable_definition(&definition_path, &definition)?;
        let marker =
            super::model::TaskMarker::from_definition(&definition, definition_path.clone());
        let desired_task = DesiredTaskSpec::canonical(
            expected_task_path.clone(),
            user_sid,
            marker,
            executable,
            cwd,
        );
        let task_changed = match observation {
            TaskObservation::Missing => {
                let observed = self.scheduler.register(&desired_task)?;
                require_no_drift(&desired_task, &observed)?;
                true
            }
            TaskObservation::Owned(observed) => {
                if semantic_drift(&desired_task, &observed.spec).is_empty() {
                    false
                } else {
                    let observed = self.scheduler.register(&desired_task)?;
                    require_no_drift(&desired_task, &observed)?;
                    true
                }
            }
            TaskObservation::Foreign { .. } => unreachable!(),
        };
        let pointer = CurrentPointer {
            schema_version: SCHEMA_VERSION,
            service_id,
            configuration_id,
            definition_path: definition_path.clone(),
            task_path: expected_task_path.clone(),
        };
        let pointer_changed = current.as_ref() != Some(&pointer);
        if pointer_changed {
            self.store.write_current(&pointer)?;
        }
        self.cleanup_definitions(&pointer)?;

        let mut changed = definition_created || task_changed || pointer_changed;
        let runtime = if options.no_start {
            self.observed_runtime_status(&definition)
        } else {
            if let Some(existing) = existing
                && existing.configuration_id != definition.configuration_id
            {
                self.prepare_configuration_replacement(existing)?;
            }
            let start = self.start_definition(&definition, &desired_task)?;
            changed |= start.changed;
            start.runtime
        };
        Ok(MutationOutput {
            command: "mcp.service.install".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed,
            action: if !was_installed {
                "installed"
            } else if changed {
                "updated"
            } else {
                "unchanged"
            }
            .to_owned(),
            service_id,
            configuration_id,
            task_path: expected_task_path,
            endpoint: definition.endpoint.mcp_url,
            registration: "installed".to_owned(),
            runtime,
        })
    }

    fn start_locked(&self) -> Result<MutationOutput, AppError> {
        let context = self.require_installed_context()?;
        require_no_drift(&context.desired, &context.observed)?;
        let result = self.start_definition(&context.definition, &context.desired)?;
        Ok(MutationOutput {
            command: "mcp.service.start".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed: result.changed,
            action: result.action,
            service_id: context.definition.service_id,
            configuration_id: context.definition.configuration_id,
            task_path: context.current.task_path,
            endpoint: context.definition.endpoint.mcp_url,
            registration: "installed".to_owned(),
            runtime: result.runtime,
        })
    }

    fn start_definition(
        &self,
        definition: &ServiceDefinition,
        desired_task: &DesiredTaskSpec,
    ) -> Result<StartResult, AppError> {
        let runtime = self.store.read_runtime().valid()?;
        let readiness = self.readiness.inspect(definition, runtime.as_ref());
        if readiness.status == ReadinessStatus::Ready {
            return Ok(StartResult {
                changed: false,
                action: "already_ready".to_owned(),
                runtime: RuntimeStatus::Ready,
                instance_id: require_ready_instance_id(&readiness)?,
            });
        }
        if readiness.status == ReadinessStatus::IdentityMismatch {
            return Err(AppError::external(
                "MCP_SERVICE_IDENTITY_MISMATCH",
                "the managed MCP endpoint is occupied by a different process identity",
            ));
        }
        let scheduler_starting = matches!(
            self.scheduler.inspect(&desired_task.task_path)?,
            TaskObservation::Owned(ObservedTask {
                scheduler_state: SchedulerState::Running | SchedulerState::Queued,
                ..
            })
        );
        let already_starting = runtime.as_ref().is_some_and(|runtime| {
            runtime.service_id == definition.service_id
                && runtime.configuration_id == definition.configuration_id
                && runtime.phase == RuntimePhase::Starting
                && (self.instance_lease_is_occupied() || scheduler_starting)
        });
        let changed = if already_starting {
            false
        } else {
            self.scheduler.run(&desired_task.task_path)?;
            true
        };
        let deadline = Instant::now() + self.start_timeout;
        loop {
            let runtime = self.store.read_runtime().valid()?;
            let readiness = self.readiness.inspect(definition, runtime.as_ref());
            if readiness.status == ReadinessStatus::Ready {
                return Ok(StartResult {
                    changed,
                    action: if already_starting {
                        "waited"
                    } else {
                        "started"
                    }
                    .to_owned(),
                    runtime: RuntimeStatus::Ready,
                    instance_id: require_ready_instance_id(&readiness)?,
                });
            }
            if readiness.status == ReadinessStatus::IdentityMismatch {
                return Err(AppError::external(
                    "MCP_SERVICE_IDENTITY_MISMATCH",
                    "the managed MCP endpoint reported a different process identity",
                ));
            }
            if Instant::now() >= deadline {
                return Err(AppError::external(
                    "MCP_SERVICE_START_TIMEOUT",
                    format!(
                        "managed MCP service did not become ready within {} ms",
                        self.start_timeout.as_millis()
                    ),
                ));
            }
            thread::sleep(self.poll_interval);
        }
    }

    fn stop_locked_output(&self) -> Result<MutationOutput, AppError> {
        let result = self.stop_locked(StopPolicy::RequireRegistration)?;
        let context = result.context.as_ref().ok_or_else(|| {
            AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "installed stop completed without service identity",
            )
        })?;
        Ok(MutationOutput {
            command: "mcp.service.stop".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed: result.changed,
            action: result.action,
            service_id: context.definition.service_id,
            configuration_id: context.definition.configuration_id,
            task_path: context.current.task_path.clone(),
            endpoint: context.definition.endpoint.mcp_url.clone(),
            registration: "installed".to_owned(),
            runtime: RuntimeStatus::Stopped,
        })
    }

    fn restart_locked(&self) -> Result<MutationOutput, AppError> {
        let initial = self.require_installed_context()?;
        require_no_drift(&initial.desired, &initial.observed)?;
        let stopped = self.stop_installed(initial)?;
        let old_instance_id = stopped.old_instance_id;
        let was_active = stopped.changed;

        let context = self.require_installed_context()?;
        require_no_drift(&context.desired, &context.observed)?;
        drop(stopped.proof_guard);
        let started = self.start_definition(&context.definition, &context.desired)?;
        if old_instance_id.is_some_and(|old| old == started.instance_id) {
            return Err(AppError::external(
                "MCP_SERVICE_RESTART_FAILED",
                "managed MCP restart returned the previous instance identity",
            ));
        }
        Ok(MutationOutput {
            command: "mcp.service.restart".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed: was_active || started.changed,
            action: if was_active { "restarted" } else { "started" }.to_owned(),
            service_id: context.definition.service_id,
            configuration_id: context.definition.configuration_id,
            task_path: context.current.task_path,
            endpoint: context.definition.endpoint.mcp_url,
            registration: "installed".to_owned(),
            runtime: RuntimeStatus::Ready,
        })
    }

    fn stop_locked(&self, policy: StopPolicy) -> Result<StopResult, AppError> {
        match self.require_installed_context() {
            Ok(context) => self.stop_installed(context),
            Err(error)
                if policy == StopPolicy::AllowExactOrphan
                    && error.code() == "MCP_SERVICE_NOT_INSTALLED" =>
            {
                self.stop_orphan()
            }
            Err(error) => Err(error),
        }
    }

    fn stop_installed(&self, context: InstalledContext) -> Result<StopResult, AppError> {
        let total_deadline = Instant::now() + self.stop_timeout;
        let runtime = self.store.read_runtime().valid()?;
        let readiness = self
            .readiness
            .inspect(&context.definition, runtime.as_ref());
        let old_instance_id = (readiness.status == ReadinessStatus::Ready)
            .then(|| readiness.instance_id)
            .flatten();
        let initial_guard = FileLease::try_acquire(&self.store.paths().instance_lock)?;
        let scheduler_active = matches!(
            context.observed.scheduler_state,
            SchedulerState::Running | SchedulerState::Queued
        ) || !self.scheduler.instances(&context.desired)?.is_empty();
        if !scheduler_active
            && readiness.status != ReadinessStatus::Ready
            && let Some(proof_guard) = initial_guard
        {
            return Ok(StopResult {
                ownership: Some(ExpectedTaskOwnership::from(&context.desired)),
                context: Some(context),
                changed: false,
                action: "already_stopped".to_owned(),
                old_instance_id,
                proof_guard,
            });
        }

        let exact_live = readiness.status == ReadinessStatus::Ready && initial_guard.is_none();
        let mut control_detail = None;
        if exact_live {
            let instance_id = require_ready_instance_id(&readiness)?;
            match self.readiness.shutdown(&context.definition, instance_id) {
                ShutdownReceipt::Accepted => {
                    let grace_deadline =
                        std::cmp::min(Instant::now() + self.stop_grace_timeout, total_deadline);
                    if let Some(guard) =
                        self.wait_for_quiescence(&context, old_instance_id, grace_deadline, None)?
                    {
                        return Ok(StopResult {
                            ownership: Some(ExpectedTaskOwnership::from(&context.desired)),
                            context: Some(context),
                            changed: true,
                            action: "stopped".to_owned(),
                            old_instance_id,
                            proof_guard: guard,
                        });
                    }
                }
                ShutdownReceipt::Failed { detail } => control_detail = Some(detail),
            }
        }

        require_no_drift(&context.desired, &context.observed).map_err(|_| {
            AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                control_detail.unwrap_or_else(|| {
                    "managed MCP task execution cannot be proven safe for Scheduler fallback"
                        .to_owned()
                }),
            )
        })?;
        let instances = self.scheduler.instances(&context.desired)?;
        let (target, held_guard) =
            self.select_stop_target(&context, runtime.as_ref(), &instances, initial_guard)?;
        self.scheduler.stop_instance(&context.desired, &target)?;
        let Some(guard) =
            self.wait_for_quiescence(&context, old_instance_id, total_deadline, held_guard)?
        else {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_TIMEOUT",
                format!(
                    "managed MCP service did not stop within {} ms",
                    self.stop_timeout.as_millis()
                ),
            ));
        };
        Ok(StopResult {
            ownership: Some(ExpectedTaskOwnership::from(&context.desired)),
            context: Some(context),
            changed: true,
            action: "forced_stopped".to_owned(),
            old_instance_id,
            proof_guard: guard,
        })
    }

    fn select_stop_target(
        &self,
        context: &InstalledContext,
        runtime: Option<&RuntimeState>,
        instances: &[SchedulerInstance],
        guard: Option<FileLease>,
    ) -> Result<(SchedulerStopTarget, Option<FileLease>), AppError> {
        if let Some(runtime) = runtime.filter(|runtime| {
            runtime.service_id == context.definition.service_id
                && runtime.configuration_id == context.definition.configuration_id
        }) {
            let mut matches = instances.iter().filter(|instance| {
                instance.state == SchedulerState::Running
                    && instance.engine_pid == Some(runtime.pid)
            });
            if let Some(instance) = matches.next()
                && matches.next().is_none()
            {
                return Ok((
                    SchedulerStopTarget::Running {
                        instance_id: instance.instance_id,
                        expected_pid: runtime.pid,
                    },
                    guard,
                ));
            }
        }

        if let Some(guard) = guard {
            let mut queued = instances.iter().filter(|instance| {
                instance.state == SchedulerState::Queued && instance.engine_pid.is_none()
            });
            if let Some(instance) = queued.next()
                && queued.next().is_none()
                && !instances.iter().any(|instance| {
                    instance.state == SchedulerState::Running || instance.engine_pid.is_some()
                })
            {
                return Ok((
                    SchedulerStopTarget::Queued {
                        instance_id: instance.instance_id,
                    },
                    Some(guard),
                ));
            }
        }
        Err(AppError::external(
            "MCP_SERVICE_STOP_UNSAFE",
            "no exact managed Task Scheduler instance can be proven for fallback",
        ))
    }

    fn wait_for_quiescence(
        &self,
        context: &InstalledContext,
        old_instance_id: Option<Uuid>,
        deadline: Instant,
        mut guard: Option<FileLease>,
    ) -> Result<Option<FileLease>, AppError> {
        loop {
            if guard.is_none() {
                guard = FileLease::try_acquire(&self.store.paths().instance_lock)?;
            }
            let observation = self.scheduler.inspect(&context.current.task_path)?;
            let TaskObservation::Owned(observed) = observation else {
                return Err(AppError::external(
                    "MCP_SERVICE_TASK_CHANGED",
                    "managed MCP task changed while stopping",
                ));
            };
            if observed.spec.marker != context.observed.spec.marker {
                return Err(AppError::external(
                    "MCP_SERVICE_TASK_CHANGED",
                    "managed MCP task marker changed while stopping",
                ));
            }
            let instances = self.scheduler.instances(&context.desired)?;
            let runtime = self.store.read_runtime().valid()?;
            let readiness = self
                .readiness
                .inspect(&context.definition, runtime.as_ref());
            let old_still_ready =
                old_instance_id.is_some_and(|old| readiness.instance_id == Some(old));
            let scheduler_inactive = !matches!(
                observed.scheduler_state,
                SchedulerState::Running | SchedulerState::Queued
            ) && instances.is_empty();
            if guard.is_some() && scheduler_inactive && !old_still_ready {
                return Ok(guard);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(self.poll_interval);
        }
    }

    fn stop_orphan(&self) -> Result<StopResult, AppError> {
        if let Some(guard) = FileLease::try_acquire(&self.store.paths().instance_lock)? {
            return Ok(StopResult {
                ownership: None,
                context: None,
                changed: false,
                action: "already_stopped".to_owned(),
                old_instance_id: None,
                proof_guard: guard,
            });
        }
        let current = match self.store.read_current() {
            Document::Valid(current) => current,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP current schema version {version}"),
                ));
            }
            Document::Missing | Document::Invalid(_) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STOP_UNSAFE",
                    "managed MCP task is missing and the occupied instance lease has no exact current identity",
                ));
            }
        };
        let expected_definition_path = self.store.paths().definition(current.configuration_id);
        if !paths_equal(&current.definition_path, &expected_definition_path) {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "managed MCP task is missing and its orphan definition cannot be proven",
            ));
        }
        let definition = match self.store.read_definition(&current.definition_path) {
            Document::Valid(definition) => definition,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP definition schema version {version}"),
                ));
            }
            Document::Missing | Document::Invalid(_) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STOP_UNSAFE",
                    "managed MCP task is missing and its orphan definition cannot be proven",
                ));
            }
        };
        if definition.service_id != current.service_id
            || definition.configuration_id != current.configuration_id
            || definition.user_sid != current_user_sid()?
            || !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime)
            || !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "managed MCP task is missing and its orphan definition cannot be proven",
            ));
        }
        let runtime = match self.store.read_runtime() {
            Document::Valid(runtime) => runtime,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP runtime schema version {version}"),
                ));
            }
            Document::Missing | Document::Invalid(_) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STOP_UNSAFE",
                    "managed MCP orphan has no durable runtime identity",
                ));
            }
        };
        let readiness = self.readiness.inspect(&definition, Some(&runtime));
        if readiness.status != ReadinessStatus::Ready {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "managed MCP orphan readiness identity cannot be proven",
            ));
        }
        let instance_id = require_ready_instance_id(&readiness)?;
        let deadline = Instant::now() + self.stop_timeout;
        match self.readiness.shutdown(&definition, instance_id) {
            ShutdownReceipt::Accepted => {}
            ShutdownReceipt::Failed { detail } => {
                return Err(AppError::external("MCP_SERVICE_STOP_UNSAFE", detail));
            }
        }
        loop {
            if let Some(guard) = FileLease::try_acquire(&self.store.paths().instance_lock)? {
                let runtime = self.store.read_runtime().valid()?;
                let readiness = self.readiness.inspect(&definition, runtime.as_ref());
                if readiness.instance_id != Some(instance_id) {
                    return Ok(StopResult {
                        ownership: None,
                        context: None,
                        changed: true,
                        action: "stopped".to_owned(),
                        old_instance_id: Some(instance_id),
                        proof_guard: guard,
                    });
                }
            }
            if Instant::now() >= deadline {
                return Err(AppError::external(
                    "MCP_SERVICE_STOP_TIMEOUT",
                    format!(
                        "managed MCP orphan did not stop within {} ms",
                        self.stop_timeout.as_millis()
                    ),
                ));
            }
            thread::sleep(self.poll_interval);
        }
    }

    fn uninstall_locked(&self) -> Result<UninstallOutput, AppError> {
        let user_sid = current_user_sid()?;
        let expected_task_path = task_path(&user_sid);
        let observation = self.scheduler.inspect(&expected_task_path)?;
        if matches!(observation, TaskObservation::Foreign { .. }) {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CONFLICT",
                "managed MCP task path is occupied by a foreign task",
            ));
        }

        let current_document = self.store.read_current();
        let runtime_document = self.store.read_runtime();
        let current = match current_document {
            Document::Valid(value) => Some(value),
            Document::Missing | Document::Invalid(_) => None,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP current schema version {version}"),
                ));
            }
        };
        let runtime = match runtime_document {
            Document::Valid(value) => Some(value),
            Document::Missing | Document::Invalid(_) => None,
            Document::UnsupportedVersion(version) => {
                return Err(AppError::external(
                    "MCP_SERVICE_STATE_INVALID",
                    format!("unsupported managed MCP runtime schema version {version}"),
                ));
            }
        };

        let task_marker = match &observation {
            TaskObservation::Owned(observed) => Some(observed.spec.marker.clone()),
            TaskObservation::Missing | TaskObservation::Foreign { .. } => None,
        };
        let trusted_service_id = task_marker
            .as_ref()
            .map(|marker| marker.service_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.service_id));
        let definition_paths = self.verified_definition_deletion_set(
            &user_sid,
            trusted_service_id,
            task_marker.as_ref(),
            current.as_ref(),
            runtime.as_ref(),
        )?;

        let mut last_definition = None;
        if let Some(marker) = &task_marker
            && let Document::Valid(definition) = self.store.read_definition(&marker.definition_path)
        {
            last_definition = Some(definition);
        }
        if last_definition.is_none()
            && let Some(pointer) = &current
            && let Document::Valid(definition) =
                self.store.read_definition(&pointer.definition_path)
        {
            last_definition = Some(definition);
        }

        let stopped = match observation {
            TaskObservation::Owned(observed) => {
                if let Some(definition) = last_definition.clone() {
                    match self.context_from_owned_task(
                        expected_task_path.clone(),
                        observed.clone(),
                        definition,
                    ) {
                        Ok(context) => self.stop_installed(context)?,
                        Err(_) => self.prove_inactive_owned_task(&expected_task_path, observed)?,
                    }
                } else {
                    self.prove_inactive_owned_task(&expected_task_path, observed)?
                }
            }
            TaskObservation::Missing => self.stop_orphan()?,
            TaskObservation::Foreign { .. } => unreachable!(),
        };

        let ownership = stopped.ownership.clone();
        let mut changed = stopped.changed;
        if let Some(ownership) = ownership {
            changed |= self.scheduler.delete_owned(&ownership)?.deleted;
            if !matches!(
                self.scheduler.inspect(&expected_task_path)?,
                TaskObservation::Missing
            ) {
                return Err(AppError::external(
                    "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                    "managed MCP task is still present after deletion",
                ));
            }
        }

        (|| -> Result<(), AppError> {
            changed |= self.store.remove_runtime()?;
            for path in definition_paths {
                changed |= self.store.remove_definition(&path)?;
            }
            changed |= self.store.remove_current()?;
            Ok(())
        })()
        .map_err(|error| {
            AppError::external(
                "MCP_SERVICE_UNINSTALL_INCOMPLETE",
                format!(
                    "managed MCP metadata cleanup failed: {}",
                    error.detail_message()
                ),
            )
        })?;

        let service_id = task_marker
            .as_ref()
            .map(|marker| marker.service_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.service_id));
        let configuration_id = task_marker
            .as_ref()
            .map(|marker| marker.configuration_id)
            .or_else(|| current.as_ref().map(|pointer| pointer.configuration_id));
        let endpoint = last_definition.map(|definition| definition.endpoint.mcp_url);
        drop(stopped.proof_guard);
        Ok(UninstallOutput {
            command: "mcp.service.uninstall".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed,
            action: if changed {
                "uninstalled"
            } else {
                "already_uninstalled"
            }
            .to_owned(),
            service_id,
            configuration_id,
            task_path: expected_task_path,
            endpoint,
            registration: "not_installed".to_owned(),
            runtime: RuntimeStatus::Stopped,
        })
    }

    fn context_from_owned_task(
        &self,
        task_path: String,
        observed: ObservedTask,
        definition: ServiceDefinition,
    ) -> Result<InstalledContext, AppError> {
        let expected_definition_path = self
            .store
            .paths()
            .definition(observed.spec.marker.configuration_id);
        if definition.user_sid != current_user_sid()?
            || observed.spec.marker.service_id != definition.service_id
            || observed.spec.marker.configuration_id != definition.configuration_id
            || !paths_equal(
                &observed.spec.marker.definition_path,
                &expected_definition_path,
            )
            || !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime)
            || !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            )
        {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "owned task identity does not match its managed definition",
            ));
        }
        let desired = DesiredTaskSpec::canonical(
            task_path.clone(),
            definition.user_sid.clone(),
            observed.spec.marker.clone(),
            definition.executable_path.clone(),
            definition.working_directory.clone(),
        );
        let current = CurrentPointer {
            schema_version: SCHEMA_VERSION,
            service_id: definition.service_id,
            configuration_id: definition.configuration_id,
            definition_path: observed.spec.marker.definition_path.clone(),
            task_path,
        };
        Ok(InstalledContext {
            current,
            definition,
            observed,
            desired,
        })
    }

    fn prove_inactive_owned_task(
        &self,
        task_path: &str,
        observed: ObservedTask,
    ) -> Result<StopResult, AppError> {
        if observed.spec.task_path != task_path {
            return Err(AppError::external(
                "MCP_SERVICE_TASK_CHANGED",
                "owned task path changed before uninstall",
            ));
        }
        let guard =
            FileLease::try_acquire(&self.store.paths().instance_lock)?.ok_or_else(|| {
                AppError::external(
                    "MCP_SERVICE_STOP_UNSAFE",
                    "owned task has no valid definition and the managed instance lease is occupied",
                )
            })?;
        if matches!(
            observed.scheduler_state,
            SchedulerState::Running | SchedulerState::Queued
        ) {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "owned task has no valid definition and is still active",
            ));
        }
        let desired = observed.spec.clone();
        if !self.scheduler.instances(&desired)?.is_empty() {
            return Err(AppError::external(
                "MCP_SERVICE_STOP_UNSAFE",
                "owned task has active Scheduler instances without a valid definition",
            ));
        }
        let ownership = ExpectedTaskOwnership::from(&observed.spec);
        Ok(StopResult {
            ownership: Some(ownership),
            context: None,
            changed: false,
            action: "already_stopped".to_owned(),
            old_instance_id: None,
            proof_guard: guard,
        })
    }

    fn verified_definition_deletion_set(
        &self,
        user_sid: &str,
        trusted_service_id: Option<Uuid>,
        marker: Option<&TaskMarker>,
        current: Option<&CurrentPointer>,
        runtime: Option<&RuntimeState>,
    ) -> Result<BTreeSet<PathBuf>, AppError> {
        let mut paths = BTreeSet::new();
        if let Some(marker) = marker {
            self.insert_definition_candidate(
                &mut paths,
                marker.configuration_id,
                &marker.definition_path,
            )?;
        }
        if let Some(current) = current {
            self.insert_definition_candidate(
                &mut paths,
                current.configuration_id,
                &current.definition_path,
            )?;
        }
        if let Some(runtime) = runtime
            && Some(runtime.service_id) == trusted_service_id
        {
            paths.insert(self.store.paths().definition(runtime.configuration_id));
        }
        for path in self.store.definition_files()? {
            match self.store.read_definition(&path) {
                Document::Valid(definition)
                    if Some(definition.service_id) == trusted_service_id
                        && definition.user_sid == user_sid
                        && paths_equal(
                            &definition.runtime_state_path,
                            &self.store.paths().runtime,
                        )
                        && paths_equal(
                            &definition.instance_lock_path,
                            &self.store.paths().instance_lock,
                        )
                        && paths_equal(
                            &path,
                            &self.store.paths().definition(definition.configuration_id),
                        ) =>
                {
                    paths.insert(path);
                }
                Document::Missing
                | Document::Invalid(_)
                | Document::UnsupportedVersion(_)
                | Document::Valid(_) => {}
            }
        }
        Ok(paths)
    }

    fn insert_definition_candidate(
        &self,
        paths: &mut BTreeSet<PathBuf>,
        configuration_id: Uuid,
        candidate: &Path,
    ) -> Result<(), AppError> {
        let expected = self.store.paths().definition(configuration_id);
        if !paths_equal(candidate, &expected) {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "managed MCP definition deletion candidate is outside the canonical store",
            ));
        }
        paths.insert(expected);
        Ok(())
    }

    fn prepare_configuration_replacement(
        &self,
        old_definition: &ServiceDefinition,
    ) -> Result<(), AppError> {
        let Some(runtime) = self.store.read_runtime().valid()? else {
            return Ok(());
        };
        if runtime.configuration_id == old_definition.configuration_id
            && matches!(
                runtime.phase,
                RuntimePhase::Starting | RuntimePhase::Ready | RuntimePhase::Stopping
            )
        {
            if !self.instance_lease_is_occupied() {
                return Ok(());
            }
            let readiness = self.readiness.inspect(old_definition, Some(&runtime));
            if readiness.status != ReadinessStatus::Ready {
                return Err(AppError::external(
                    "MCP_SERVICE_RESTART_REQUIRED",
                    "old managed MCP instance cannot be identified for controlled replacement",
                ));
            }
            match self.readiness.shutdown(old_definition, runtime.instance_id) {
                ShutdownReceipt::Accepted => {}
                ShutdownReceipt::Failed { detail } => {
                    return Err(AppError::external("MCP_SERVICE_RESTART_REQUIRED", detail));
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match FileLease::try_acquire(&self.store.paths().instance_lock)? {
                    Some(lease) => {
                        drop(lease);
                        return Ok(());
                    }
                    None if Instant::now() < deadline => thread::sleep(self.poll_interval),
                    None => {
                        return Err(AppError::external(
                            "MCP_SERVICE_RESTART_REQUIRED",
                            "old managed MCP instance did not release the instance lease",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn status_snapshot(&self) -> StatusOutput {
        let user_sid = match current_user_sid() {
            Ok(value) => value,
            Err(error) => {
                let mut output = StatusOutput::not_installed("\\AIHelper Managed MCP".to_owned());
                output.registration.status = RegistrationStatus::SchedulerError;
                output.registration.diagnostic_code = Some(error.code().to_owned());
                output.scheduler.state = SchedulerState::Error;
                output.scheduler.diagnostic_code = Some(error.code().to_owned());
                return output;
            }
        };
        let expected_task_path = task_path(&user_sid);
        let mut output = StatusOutput::not_installed(expected_task_path.clone());
        self.observe_lifecycle(&mut output);
        let current = match self.store.read_current() {
            Document::Missing => None,
            Document::Valid(value) => Some(value),
            Document::Invalid(_) | Document::UnsupportedVersion(_) => {
                output.registration.status = RegistrationStatus::ConfigurationDrift;
                output.registration.diagnostic_code = Some("MCP_SERVICE_STATE_INVALID".to_owned());
                output.drift.push(invalid_drift("current.invalid"));
                None
            }
        };
        if let Some(current) = &current {
            output.registration.service_id = Some(current.service_id);
            output.registration.configuration_id = Some(current.configuration_id);
            output.registration.definition_path =
                Some(current.definition_path.to_string_lossy().into_owned());
        }

        let observation = match self.scheduler.inspect(&expected_task_path) {
            Ok(observation) => observation,
            Err(error) => {
                let (hresult, hresult_hex) = scheduler_error_values(&error);
                output.registration.status = RegistrationStatus::SchedulerError;
                output.registration.diagnostic_code = Some(error.code().to_owned());
                output.scheduler.state = SchedulerState::Error;
                output.scheduler.diagnostic_code = Some(error.code().to_owned());
                output.scheduler.hresult = hresult;
                output.scheduler.hresult_hex = hresult_hex;
                output.sort_drift();
                return output;
            }
        };
        let observed = match observation {
            TaskObservation::Missing => {
                if current.is_some() {
                    output.drift.push(DriftEntry::missing(
                        "task.missing",
                        Some(expected_task_path),
                    ));
                }
                output.sort_drift();
                return output;
            }
            TaskObservation::Foreign { .. } => {
                output.registration.status = RegistrationStatus::ConfigurationDrift;
                output.registration.diagnostic_code = Some("MCP_SERVICE_TASK_CONFLICT".to_owned());
                output.drift.push(invalid_drift("task.ownership"));
                output.scheduler.state = SchedulerState::Unknown;
                output.sort_drift();
                return output;
            }
            TaskObservation::Owned(observed) => observed,
        };
        apply_scheduler_section(&mut output, &observed);
        output.registration.service_id = Some(observed.spec.marker.service_id);
        output.registration.configuration_id = Some(observed.spec.marker.configuration_id);
        output.registration.definition_path = Some(
            observed
                .spec
                .marker
                .definition_path
                .to_string_lossy()
                .into_owned(),
        );
        let expected_definition_path = self
            .store
            .paths()
            .definition(observed.spec.marker.configuration_id);
        let definition = if !paths_equal(
            &observed.spec.marker.definition_path,
            &expected_definition_path,
        ) {
            output.registration.status = RegistrationStatus::ConfigurationDrift;
            output.registration.diagnostic_code = Some("MCP_SERVICE_STATE_INVALID".to_owned());
            output.drift.push(invalid_drift("definition.path"));
            None
        } else {
            match self
                .store
                .read_definition(&observed.spec.marker.definition_path)
            {
                Document::Valid(definition) => Some(definition),
                Document::Missing | Document::Invalid(_) | Document::UnsupportedVersion(_) => {
                    output.registration.status = RegistrationStatus::ConfigurationDrift;
                    output.registration.diagnostic_code =
                        Some("MCP_SERVICE_STATE_INVALID".to_owned());
                    output.drift.push(invalid_drift("definition.invalid"));
                    None
                }
            }
        };
        if let Some(definition) = &definition {
            if definition.user_sid != user_sid {
                output.drift.push(DriftEntry::mismatch(
                    "definition.user_sid",
                    &user_sid,
                    &definition.user_sid,
                ));
            }
            if !paths_equal(&definition.runtime_state_path, &self.store.paths().runtime) {
                output.drift.push(DriftEntry::mismatch(
                    "definition.runtime_state_path",
                    self.store.paths().runtime.to_string_lossy(),
                    definition.runtime_state_path.to_string_lossy(),
                ));
            }
            if !paths_equal(
                &definition.instance_lock_path,
                &self.store.paths().instance_lock,
            ) {
                output.drift.push(DriftEntry::mismatch(
                    "definition.instance_lock_path",
                    self.store.paths().instance_lock.to_string_lossy(),
                    definition.instance_lock_path.to_string_lossy(),
                ));
            }
            let desired = DesiredTaskSpec::canonical(
                expected_task_path,
                user_sid.clone(),
                observed.spec.marker.clone(),
                definition.executable_path.clone(),
                definition.working_directory.clone(),
            );
            output
                .drift
                .extend(semantic_drift(&desired, &observed.spec));
            match current.as_ref() {
                None => output.drift.push(DriftEntry::missing(
                    "current.missing",
                    Some(definition.configuration_id.to_string()),
                )),
                Some(current)
                    if current.service_id != definition.service_id
                        || current.configuration_id != definition.configuration_id =>
                {
                    output.drift.push(DriftEntry::mismatch(
                        "current.configuration_id",
                        definition.configuration_id.to_string(),
                        current.configuration_id.to_string(),
                    ));
                }
                Some(current) if current.task_path != observed.spec.task_path => {
                    output.drift.push(DriftEntry::mismatch(
                        "current.task_path",
                        &observed.spec.task_path,
                        &current.task_path,
                    ));
                }
                Some(current)
                    if !paths_equal(
                        &current.definition_path,
                        &observed.spec.marker.definition_path,
                    ) =>
                {
                    output.drift.push(DriftEntry::mismatch(
                        "current.definition_path",
                        observed.spec.marker.definition_path.to_string_lossy(),
                        current.definition_path.to_string_lossy(),
                    ));
                }
                Some(_) => {}
            }
            self.observe_runtime(&mut output, definition, &observed);
        }
        if !output.drift.iter().any(is_registration_drift) {
            output.registration.status = RegistrationStatus::Installed;
            output.registration.diagnostic_code = None;
        } else {
            output.registration.status = RegistrationStatus::ConfigurationDrift;
            output.registration.diagnostic_code =
                Some("MCP_SERVICE_CONFIGURATION_DRIFT".to_owned());
        }
        output.sort_drift();
        output
    }

    fn observe_runtime(
        &self,
        output: &mut StatusOutput,
        definition: &ServiceDefinition,
        observed: &ObservedTask,
    ) {
        let runtime = match self.store.read_runtime() {
            Document::Missing => None,
            Document::Valid(value) => Some(value),
            Document::Invalid(_) | Document::UnsupportedVersion(_) => {
                output.runtime.diagnostic_code = Some("MCP_SERVICE_STATE_INVALID".to_owned());
                output.drift.push(invalid_drift("runtime.invalid"));
                None
            }
        };
        if let Some(runtime) = &runtime {
            output.runtime.service_id = Some(runtime.service_id);
            output.runtime.configuration_id = Some(runtime.configuration_id);
            output.runtime.version = Some(runtime.version.clone());
            output.runtime.instance_id = Some(runtime.instance_id);
            output.runtime.pid = Some(runtime.pid);
            output.runtime.endpoint = Some(runtime.endpoint.clone());
            output.runtime.started_at = Some(runtime.started_at.clone());
            output.runtime.updated_at = Some(runtime.updated_at.clone());
        }
        output.readiness = self.readiness.inspect(definition, runtime.as_ref());
        let instance_occupied = self.instance_lease_is_occupied();
        output.runtime.status = reduce_runtime(
            runtime.as_ref(),
            &output.readiness,
            SchedulerRuntimeEvidence::from_observed(observed),
            output.lifecycle.status,
            instance_occupied,
        );
        output.runtime.diagnostic_code = match output.runtime.status {
            RuntimeStatus::IdentityMismatch => Some("MCP_SERVICE_IDENTITY_MISMATCH".to_owned()),
            RuntimeStatus::Failed | RuntimeStatus::RestartBackoff => runtime
                .as_ref()
                .and_then(|runtime| runtime.last_exit.as_ref())
                .and_then(|exit| exit.diagnostic_code.clone()),
            _ => output.runtime.diagnostic_code.take(),
        };
    }

    fn observe_lifecycle(&self, output: &mut StatusOutput) {
        if !self.store.paths().lifecycle_lock.exists() {
            if matches!(
                self.store.read_lifecycle(),
                Document::Invalid(_) | Document::UnsupportedVersion(_)
            ) {
                output.drift.push(invalid_drift("lifecycle.invalid"));
            }
            return;
        }
        match FileLease::try_acquire(&self.store.paths().lifecycle_lock) {
            Ok(Some(lease)) => {
                drop(lease);
                if matches!(
                    self.store.read_lifecycle(),
                    Document::Invalid(_) | Document::UnsupportedVersion(_)
                ) {
                    output.drift.push(invalid_drift("lifecycle.invalid"));
                }
            }
            Ok(None) => {
                output.lifecycle.status = LifecycleStatus::Busy;
                match self.store.read_lifecycle() {
                    Document::Valid(state) if state.state == LifecycleStateKind::Active => {
                        output.lifecycle.operation = Some(LifecycleOperationSection {
                            operation_id: state.operation_id,
                            operation: state.operation,
                            pid: state.pid,
                            service_id: state.service_id,
                            started_at: state.started_at,
                        });
                    }
                    Document::Invalid(_) | Document::UnsupportedVersion(_) => {
                        output.drift.push(invalid_drift("lifecycle.invalid"));
                    }
                    Document::Missing | Document::Valid(_) => {}
                }
            }
            Err(_) => {
                output.lifecycle.status = LifecycleStatus::Busy;
            }
        }
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
            observed,
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
        let readiness = self.readiness.inspect(definition, runtime.as_ref());
        reduce_runtime(
            runtime.as_ref(),
            &readiness,
            SchedulerRuntimeEvidence::unverified(SchedulerState::Ready),
            LifecycleStatus::Busy,
            self.instance_lease_is_occupied(),
        )
    }

    fn instance_lease_is_occupied(&self) -> bool {
        if !self.store.paths().instance_lock.exists() {
            return false;
        }
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

fn require_ready_instance_id(readiness: &ReadinessSection) -> Result<Uuid, AppError> {
    readiness.instance_id.ok_or_else(|| {
        AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            "ready managed MCP response has no instance identity",
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn configuration_matches(
    existing: &ServiceDefinition,
    user_sid: &str,
    executable: &Path,
    cwd: &Path,
    config_dir: &Path,
    runtime_path: &Path,
    instance_lock_path: &Path,
    endpoint: &ServiceEndpoint,
    server: &ServerDefinition,
) -> bool {
    existing.schema_version == SCHEMA_VERSION
        && existing.task_spec_version == TASK_SPEC_VERSION
        && existing.user_sid == user_sid
        && paths_equal(&existing.executable_path, executable)
        && paths_equal(&existing.working_directory, cwd)
        && paths_equal(&existing.config_directory, config_dir)
        && paths_equal(&existing.runtime_state_path, runtime_path)
        && paths_equal(&existing.instance_lock_path, instance_lock_path)
        && existing.expected_version == env!("CARGO_PKG_VERSION")
        && &existing.endpoint == endpoint
        && &existing.server == server
}

fn require_no_drift(desired: &DesiredTaskSpec, observed: &ObservedTask) -> Result<(), AppError> {
    let drift = semantic_drift(desired, &observed.spec);
    if drift.is_empty() {
        Ok(())
    } else {
        Err(AppError::external(
            "MCP_SERVICE_CONFIGURATION_DRIFT",
            format!(
                "Task Scheduler readback has {} drifted properties",
                drift.len()
            ),
        ))
    }
}

fn apply_scheduler_section(output: &mut StatusOutput, observed: &ObservedTask) {
    output.scheduler = SchedulerSection {
        state: observed.scheduler_state,
        last_result: observed.last_result,
        last_result_hex: observed.last_result.map(super::model::hresult_hex),
        last_run_at: observed.last_run_at.clone(),
        diagnostic_code: None,
        hresult: None,
        hresult_hex: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SchedulerRuntimeEvidence {
    state: SchedulerState,
    last_result: Option<i32>,
    canonical_restart_policy: bool,
}

impl SchedulerRuntimeEvidence {
    fn from_observed(observed: &ObservedTask) -> Self {
        Self {
            state: observed.scheduler_state,
            last_result: observed.last_result,
            canonical_restart_policy: has_canonical_restart_policy(&observed.spec),
        }
    }

    fn unverified(state: SchedulerState) -> Self {
        Self {
            state,
            last_result: None,
            canonical_restart_policy: false,
        }
    }
}

fn reduce_runtime(
    runtime: Option<&RuntimeState>,
    readiness: &ReadinessSection,
    scheduler: SchedulerRuntimeEvidence,
    lifecycle: LifecycleStatus,
    instance_occupied: bool,
) -> RuntimeStatus {
    if readiness.status == ReadinessStatus::IdentityMismatch {
        return RuntimeStatus::IdentityMismatch;
    }
    if runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Stopping) {
        return RuntimeStatus::Stopping;
    }
    if readiness.status == ReadinessStatus::Ready {
        return RuntimeStatus::Ready;
    }
    if instance_occupied || scheduler.state == SchedulerState::Running {
        return if runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Starting) {
            RuntimeStatus::Starting
        } else {
            RuntimeStatus::RunningNotReady
        };
    }
    if scheduler.state == SchedulerState::Queued {
        if lifecycle == LifecycleStatus::Idle
            && scheduler.canonical_restart_policy
            && scheduler.last_result.is_some_and(|result| result != 0)
            && runtime.is_some_and(|runtime| {
                runtime.phase == RuntimePhase::Failed
                    && runtime
                        .last_exit
                        .as_ref()
                        .is_some_and(|exit| exit.kind != ExitKind::Clean && exit.exit_code != 0)
            })
        {
            return RuntimeStatus::RestartBackoff;
        }
        return RuntimeStatus::Starting;
    }
    if runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Failed) {
        return RuntimeStatus::Failed;
    }
    if lifecycle == LifecycleStatus::Busy
        && runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Starting)
    {
        return RuntimeStatus::Starting;
    }
    RuntimeStatus::Stopped
}

fn invalid_drift(field: &str) -> DriftEntry {
    DriftEntry {
        field: field.to_owned(),
        kind: DriftKind::Invalid,
        expected: None,
        actual: None,
        diagnostic_code: "MCP_SERVICE_STATE_INVALID".to_owned(),
    }
}

fn is_registration_drift(entry: &DriftEntry) -> bool {
    [
        "task.",
        "current.",
        "definition.",
        "registration.",
        "principal.",
        "trigger.",
        "triggers.",
        "action.",
        "actions.",
        "settings.",
    ]
    .iter()
    .any(|prefix| entry.field.starts_with(prefix))
}

fn scheduler_error_values(error: &AppError) -> (Option<i32>, Option<String>) {
    let detail = error.detail_message();
    let Some(suffix) = detail.split("hresult=").nth(1) else {
        return (None, None);
    };
    let mut fields = suffix.trim_end_matches(')').split_whitespace();
    let value = fields.next().and_then(|value| value.parse::<i32>().ok());
    let hex = fields
        .next()
        .filter(|value| {
            value.len() == 10
                && value.starts_with("0x")
                && value[2..]
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
        })
        .map(str::to_owned);
    (value, hex)
}

#[cfg(test)]
mod tests;
