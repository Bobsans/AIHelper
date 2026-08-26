//! Observing what the service currently is, and reducing scheduler and runtime
//! evidence into one reported state.
//!
//! Status is the only read-only operation, and the only one that must answer
//! even when every layer below it disagrees with the others.

use super::*;

pub(super) fn require_ready_instance_id(readiness: &ReadinessSection) -> Result<Uuid, AppError> {
    readiness.instance_id.ok_or_else(|| {
        AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            "ready managed MCP response has no instance identity",
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn configuration_matches(
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

pub(super) fn require_no_drift(
    desired: &DesiredTaskSpec,
    observed: &ObservedTask,
) -> Result<(), AppError> {
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

pub(super) fn apply_scheduler_section(output: &mut StatusOutput, observed: &ObservedTask) {
    output.scheduler = SchedulerSection {
        state: observed.scheduler_state,
        last_result: observed.last_result,
        last_result_hex: observed
            .last_result
            .map(crate::mcp_service::model::hresult_hex),
        last_run_at: observed.last_run_at.clone(),
        diagnostic_code: None,
        hresult: None,
        hresult_hex: None,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SchedulerRuntimeEvidence {
    pub(super) state: SchedulerState,
    pub(super) canonical_restart_policy: bool,
}

impl SchedulerRuntimeEvidence {
    pub(super) fn from_observed(observed: &ObservedTask) -> Self {
        Self {
            state: observed.scheduler_state,
            canonical_restart_policy: has_canonical_restart_policy(&observed.spec),
        }
    }

    pub(super) fn unverified(state: SchedulerState) -> Self {
        Self {
            state,
            canonical_restart_policy: false,
        }
    }
}

pub(super) fn reduce_runtime(
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
    let fatal_runtime = runtime.is_some_and(|runtime| {
        runtime.phase == RuntimePhase::Failed
            && runtime
                .last_exit
                .as_ref()
                .is_some_and(|exit| exit.kind != ExitKind::Clean && exit.exit_code != 0)
    });
    if scheduler.state == SchedulerState::Running
        && lifecycle == LifecycleStatus::Idle
        && scheduler.canonical_restart_policy
        && fatal_runtime
        && !instance_occupied
    {
        return RuntimeStatus::RestartBackoff;
    }
    if instance_occupied || scheduler.state == SchedulerState::Running {
        return if runtime.is_some_and(|runtime| runtime.phase == RuntimePhase::Starting) {
            RuntimeStatus::Starting
        } else {
            RuntimeStatus::RunningNotReady
        };
    }
    if scheduler.state == SchedulerState::Queued {
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

pub(super) fn invalid_drift(field: &str) -> DriftEntry {
    DriftEntry {
        field: field.to_owned(),
        kind: DriftKind::Invalid,
        expected: None,
        actual: None,
        diagnostic_code: "MCP_SERVICE_STATE_INVALID".to_owned(),
    }
}

pub(super) fn is_registration_drift(entry: &DriftEntry) -> bool {
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

pub(super) fn scheduler_error_values(error: &AppError) -> (Option<i32>, Option<String>) {
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

impl<S: SchedulerAdapter, R: RuntimeControl> LifecycleService<S, R> {
    pub(super) fn status_snapshot(&self) -> StatusOutput {
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
                if let Some(current) = current.as_ref() {
                    match self.require_pointer_definition(current) {
                        Ok(definition)
                            if definition.user_sid == user_sid
                                && current.task_path == expected_task_path =>
                        {
                            self.observe_runtime(
                                &mut output,
                                &definition,
                                SchedulerRuntimeEvidence::unverified(SchedulerState::Error),
                            );
                        }
                        Ok(_) | Err(_) => {
                            output.drift.push(invalid_drift("definition.invalid"));
                        }
                    }
                }
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
            self.observe_runtime(
                &mut output,
                definition,
                SchedulerRuntimeEvidence::from_observed(&observed),
            );
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

    pub(super) fn observe_runtime(
        &self,
        output: &mut StatusOutput,
        definition: &ServiceDefinition,
        scheduler: SchedulerRuntimeEvidence,
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
        output.readiness = self.readiness.inspect(definition, runtime.as_ref(), false);
        let instance_occupied = self.instance_lease_is_occupied();
        output.runtime.status = reduce_runtime(
            runtime.as_ref(),
            &output.readiness,
            scheduler,
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

    pub(super) fn observe_lifecycle(&self, output: &mut StatusOutput) {
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
}
