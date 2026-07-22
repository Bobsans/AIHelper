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
        CurrentPointer, DriftEntry, DriftKind, LifecycleOperation, LifecycleState,
        LifecycleStateKind, MutationOutput, RuntimePhase, RuntimeState, RuntimeStatus,
        SCHEMA_VERSION, ServerDefinition, ServiceDefinition, ServiceEndpoint, TASK_SPEC_VERSION,
        now_timestamp,
    },
    output::{
        LifecycleOperationSection, LifecycleStatus, ReadinessSection, ReadinessStatus,
        RegistrationStatus, SchedulerSection, SchedulerState, StatusOutput, emit_mutation,
        emit_status,
    },
    paths::{ServicePaths, current_user_sid, normalize_absolute_path, paths_equal, task_path},
    readiness::{HttpReadinessProbe, ReadinessProbe},
    scheduler::{DesiredTaskSpec, ObservedTask, SchedulerAdapter, TaskObservation, semantic_drift},
    store::{Document, ServiceStore},
};

const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const START_TIMEOUT: Duration = Duration::from_secs(15);
const START_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
            ServiceCommand::Status { options } => {
                let output = service.status();
                emit_status(&output, options.output, options.quiet)
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

pub struct LifecycleService<S, R> {
    store: ServiceStore,
    scheduler: S,
    readiness: R,
    start_timeout: Duration,
    poll_interval: Duration,
}

impl<S: SchedulerAdapter, R: ReadinessProbe> LifecycleService<S, R> {
    pub fn new(paths: ServicePaths, scheduler: S, readiness: R) -> Self {
        Self {
            store: ServiceStore::new(paths),
            scheduler,
            readiness,
            start_timeout: START_TIMEOUT,
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
        let drift = semantic_drift(&desired, &observed.spec);
        if !drift.is_empty() {
            return Err(AppError::external(
                "MCP_SERVICE_CONFIGURATION_DRIFT",
                format!("managed MCP task has {} drifted properties", drift.len()),
            ));
        }
        let result = self.start_definition(&definition, &desired)?;
        Ok(MutationOutput {
            command: "mcp.service.start".to_owned(),
            schema_version: SCHEMA_VERSION,
            changed: result.changed,
            action: result.action,
            service_id: definition.service_id,
            configuration_id: definition.configuration_id,
            task_path: current.task_path,
            endpoint: definition.endpoint.mcp_url,
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
            request_control_shutdown(old_definition, runtime.instance_id)?;
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
            self.observe_runtime(&mut output, definition, observed.scheduler_state);
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
        scheduler_state: SchedulerState,
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
            scheduler_state,
            output.lifecycle.status,
            instance_occupied,
        );
        output.runtime.diagnostic_code = match output.runtime.status {
            RuntimeStatus::IdentityMismatch => Some("MCP_SERVICE_IDENTITY_MISMATCH".to_owned()),
            RuntimeStatus::Failed => runtime
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
            SchedulerState::Ready,
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
        let _lease =
            FileLease::acquire(&self.store.paths().lifecycle_lock, LIFECYCLE_LOCK_TIMEOUT)?;
        let mut state = LifecycleState::active(operation, service_id);
        self.store.write_lifecycle(&state)?;
        match callback() {
            Ok(value) => {
                state.state = LifecycleStateKind::Completed;
                state.finished_at = Some(now_timestamp());
                self.store.write_lifecycle(&state)?;
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

struct StartResult {
    changed: bool,
    action: String,
    runtime: RuntimeStatus,
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

fn request_control_shutdown(
    definition: &ServiceDefinition,
    instance_id: Uuid,
) -> Result<(), AppError> {
    let url = format!(
        "http://127.0.0.1:{}/control/shutdown",
        definition.endpoint.port
    );
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|client| {
            client
                .post(url)
                .json(&serde_json::json!({"instance_id": instance_id}))
                .send()
        })
        .map_err(|error| {
            AppError::external(
                "MCP_SERVICE_RESTART_REQUIRED",
                format!("failed to request controlled MCP shutdown: {error}"),
            )
        })?;
    if response.status() != reqwest::StatusCode::ACCEPTED {
        return Err(AppError::external(
            "MCP_SERVICE_RESTART_REQUIRED",
            format!(
                "controlled MCP shutdown returned HTTP {}",
                response.status().as_u16()
            ),
        ));
    }
    Ok(())
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

fn reduce_runtime(
    runtime: Option<&RuntimeState>,
    readiness: &ReadinessSection,
    scheduler: SchedulerState,
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
    if instance_occupied || matches!(scheduler, SchedulerState::Running) {
        return if runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Starting) {
            RuntimeStatus::Starting
        } else {
            RuntimeStatus::RunningNotReady
        };
    }
    if matches!(scheduler, SchedulerState::Queued)
        && runtime.is_some_and(|runtime| {
            matches!(runtime.phase, RuntimePhase::Failed | RuntimePhase::Stopped)
        })
    {
        return RuntimeStatus::RestartBackoff;
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
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use tempfile::TempDir;

    use super::*;
    use crate::{cli::GlobalOptions, output::OutputMode};

    struct FakeScheduler {
        observation: Mutex<TaskObservation>,
        register_count: AtomicUsize,
        run_count: AtomicUsize,
        fail_inspect: bool,
    }

    impl FakeScheduler {
        fn missing() -> Self {
            Self {
                observation: Mutex::new(TaskObservation::Missing),
                register_count: AtomicUsize::new(0),
                run_count: AtomicUsize::new(0),
                fail_inspect: false,
            }
        }

        fn foreign() -> Self {
            Self {
                observation: Mutex::new(TaskObservation::Foreign {
                    source: Some("Other".to_owned()),
                    uri: None,
                }),
                ..Self::missing()
            }
        }
    }

    impl SchedulerAdapter for FakeScheduler {
        fn inspect(&self, _task_path: &str) -> Result<TaskObservation, AppError> {
            if self.fail_inspect {
                return Err(AppError::external(
                    "MCP_SERVICE_SCHEDULER_FAILED",
                    "fake scheduler failure",
                ));
            }
            Ok(self
                .observation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone())
        }

        fn register(&self, desired: &DesiredTaskSpec) -> Result<ObservedTask, AppError> {
            self.register_count.fetch_add(1, Ordering::Relaxed);
            let observed = ObservedTask {
                spec: desired.clone(),
                scheduler_state: SchedulerState::Ready,
                last_result: Some(0),
                last_run_at: None,
            };
            *self
                .observation
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                TaskObservation::Owned(observed.clone());
            Ok(observed)
        }

        fn run(
            &self,
            _task_path: &str,
        ) -> Result<super::super::scheduler::SchedulerRunReceipt, AppError> {
            self.run_count.fetch_add(1, Ordering::Relaxed);
            Ok(super::super::scheduler::SchedulerRunReceipt { submitted: true })
        }
    }

    #[derive(Clone)]
    struct FakeReadiness {
        section: ReadinessSection,
    }

    impl ReadinessProbe for FakeReadiness {
        fn inspect(
            &self,
            _definition: &ServiceDefinition,
            _runtime: Option<&RuntimeState>,
        ) -> ReadinessSection {
            self.section.clone()
        }
    }

    fn not_ready() -> FakeReadiness {
        FakeReadiness {
            section: ReadinessSection {
                status: ReadinessStatus::NotReady,
                http_status: None,
                version: None,
                instance_id: None,
                pid: None,
                diagnostic_code: Some("MCP_SERVICE_START_TIMEOUT".to_owned()),
            },
        }
    }

    fn install_options(no_start: bool) -> InstallOptions {
        InstallOptions {
            no_start,
            port: 8787,
            max_active: 32,
            default_timeout_ms: 300_000,
            options: GlobalOptions {
                output: OutputMode::Json,
                quiet: false,
                limit: None,
            },
        }
    }

    #[cfg(windows)]
    #[test]
    fn no_start_install_is_idempotent_and_publishes_verified_pointer() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
        let first = service.install(&install_options(true)).unwrap();
        assert!(first.changed);
        assert_eq!(first.action, "installed");
        assert_eq!(service.scheduler.register_count.load(Ordering::Relaxed), 1);
        let second = service.install(&install_options(true)).unwrap();
        assert!(!second.changed);
        assert_eq!(second.action, "unchanged");
        assert_eq!(service.scheduler.register_count.load(Ordering::Relaxed), 1);
        let Document::Valid(pointer) = service.store.read_current() else {
            panic!("current pointer should be valid")
        };
        let Document::Valid(definition) = service.store.read_definition(&pointer.definition_path)
        else {
            panic!("definition should be valid")
        };
        assert_eq!(definition.configuration_id, pointer.configuration_id);
    }

    #[cfg(windows)]
    #[test]
    fn malformed_current_blocks_install_without_overwrite() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        std::fs::create_dir_all(&paths.base_dir).unwrap();
        std::fs::write(&paths.current, b"malformed").unwrap();
        let service = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
        let error = service.install(&install_options(true)).unwrap_err();
        assert_eq!(error.code(), "MCP_SERVICE_STATE_INVALID");
        assert_eq!(std::fs::read(&paths.current).unwrap(), b"malformed");
    }

    #[cfg(windows)]
    #[test]
    fn status_is_read_only_for_absent_service_and_reports_foreign_task() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let missing = LifecycleService::new(paths.clone(), FakeScheduler::missing(), not_ready());
        let status = missing.status();
        assert_eq!(status.registration.status, RegistrationStatus::NotInstalled);
        assert!(!paths.base_dir.exists());

        let foreign = LifecycleService::new(paths, FakeScheduler::foreign(), not_ready());
        let status = foreign.status();
        assert_eq!(
            status.registration.status,
            RegistrationStatus::ConfigurationDrift
        );
        assert_eq!(
            status.registration.diagnostic_code.as_deref(),
            Some("MCP_SERVICE_TASK_CONFLICT")
        );
    }

    #[cfg(windows)]
    #[test]
    fn start_does_not_treat_scheduler_submission_as_readiness() {
        let temp = TempDir::new().unwrap();
        let paths = ServicePaths::from_base(temp.path().join("managed")).unwrap();
        let mut service = LifecycleService::new(paths, FakeScheduler::missing(), not_ready());
        service.install(&install_options(true)).unwrap();
        service.start_timeout = Duration::from_millis(2);
        service.poll_interval = Duration::from_millis(1);
        let error = service.start().unwrap_err();
        assert_eq!(error.code(), "MCP_SERVICE_START_TIMEOUT");
        assert_eq!(service.scheduler.run_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn scheduler_hresult_is_preserved_for_structured_status() {
        let error = AppError::external(
            "MCP_SERVICE_SCHEDULER_FAILED",
            "Task Scheduler operation 'inspect' failed (hresult=-2147024891 0x80070005)",
        );
        assert_eq!(
            scheduler_error_values(&error),
            (Some(-2_147_024_891), Some("0x80070005".to_owned()))
        );
    }
}
