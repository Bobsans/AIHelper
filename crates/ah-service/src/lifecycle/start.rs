//! Starting the service and waiting for it to report itself ready.

use super::*;

impl<S: ServiceScheduler, R: RuntimeControl> LifecycleService<S, R> {
    pub(crate) fn start_locked(&self) -> Result<MutationOutput, AppError> {
        let context = self.require_installed_context()?;
        self.require_no_drift(&context.desired, &context.observed)?;
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

    pub(crate) fn start_definition(
        &self,
        definition: &ServiceDefinition,
        desired: &ServiceSpec,
    ) -> Result<StartResult, AppError> {
        let runtime = self.store.read_runtime().valid()?;
        let readiness = self.readiness.inspect(definition, runtime.as_ref(), false);
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
            self.scheduler.inspect(&desired.id)?,
            ServiceObservation::Owned(observed) if matches!(
                observed.state,
                SchedulerState::Running | SchedulerState::Queued
            )
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
            self.scheduler.run(&desired.id)?;
            true
        };
        let deadline = Instant::now() + self.start_timeout;
        loop {
            let runtime = self.store.read_runtime().valid()?;
            let readiness = self.readiness.inspect(definition, runtime.as_ref(), false);
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
}
