//! Stopping the service, and the restart that is a stop followed by a start.
//!
//! Includes the orphan path: a running server whose registration is already
//! gone still has to be stopped, and can only be identified by its lease.

use super::*;

impl<S: SchedulerAdapter, R: RuntimeControl> LifecycleService<S, R> {
    pub(super) fn stop_locked_output(&self) -> Result<MutationOutput, AppError> {
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

    pub(super) fn restart_locked(&self) -> Result<MutationOutput, AppError> {
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

    pub(super) fn stop_locked(&self, policy: StopPolicy) -> Result<StopResult, AppError> {
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

    pub(super) fn stop_installed(&self, context: InstalledContext) -> Result<StopResult, AppError> {
        let total_deadline = Instant::now() + self.stop_timeout;
        let runtime = self.store.read_runtime().valid()?;
        let readiness = self
            .readiness
            .inspect(&context.definition, runtime.as_ref(), false);
        let old_instance_id = (readiness.status == ReadinessStatus::Ready)
            .then_some(readiness.instance_id)
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

    pub(super) fn select_stop_target(
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

    pub(super) fn wait_for_quiescence(
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
                .inspect(&context.definition, runtime.as_ref(), true);
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

    pub(super) fn stop_orphan(&self) -> Result<StopResult, AppError> {
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
        let readiness = self.readiness.inspect(&definition, Some(&runtime), false);
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
                let readiness = self.readiness.inspect(&definition, runtime.as_ref(), false);
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
}
