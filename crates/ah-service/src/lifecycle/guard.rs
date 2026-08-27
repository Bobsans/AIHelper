//! The managed service seen as something an update has to hold still.
//!
//! Three functions used to sit in the lifecycle root as `pub(crate)` exports
//! named `*_while_locked`, and the updater called them by name. They are the
//! same three operations; what changed is that the updater now asks for them
//! through `ah_updater::service::ServiceGuard`, so it no longer knows this module
//! exists.

use super::*;

use ah_updater::service::{ServiceGuard, ServiceHold, ServiceState};

/// The real managed MCP service.
pub struct ManagedMcpGuard;

impl ServiceGuard for ManagedMcpGuard {
    fn hold(&self, timeout: Duration) -> Result<ServiceHold, AppError> {
        let paths = ServicePaths::discover()?;
        lock::acquire(&paths.lifecycle_lock, timeout)
    }

    fn capture(&self, _hold: &ServiceHold) -> Result<ServiceState, AppError> {
        Ok(state_from_status(&platform_service()?.status()))
    }

    fn stop(&self, _hold: &ServiceHold) -> Result<bool, AppError> {
        let service = platform_service()?;
        Ok(service.stop_locked(StopPolicy::AllowExactOrphan)?.changed)
    }

    fn restore(&self, _hold: &ServiceHold, state: ServiceState) -> Result<(), AppError> {
        if !state.was_running {
            return Ok(());
        }
        let service = platform_service()?;
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
}

/// Whether an update has to put this service back, and which instance it is
/// putting back.
///
/// Any of the three layers reporting activity counts as running. The scheduler
/// can say `Queued` before a runtime exists, and the runtime can be `Starting`
/// before readiness answers; treating readiness as the only authority would
/// leave a service that was mid-start stopped after the update.
///
/// The instance id is kept only when it was running, because `restore` compares
/// it against the one that comes back to prove the service actually restarted
/// rather than never having stopped. A stale id from a service that was already
/// down would make that comparison lie.
fn state_from_status(status: &StatusOutput) -> ServiceState {
    let was_running = status.readiness.status == ReadinessStatus::Ready
        || matches!(
            status.scheduler.state,
            SchedulerState::Running | SchedulerState::Queued
        )
        || matches!(
            status.runtime.status,
            RuntimeStatus::Starting | RuntimeStatus::RunningNotReady | RuntimeStatus::Ready
        );
    ServiceState {
        was_running,
        previous_instance_id: status
            .readiness
            .instance_id
            .or(status.runtime.instance_id)
            .filter(|_| was_running),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> StatusOutput {
        StatusOutput::not_installed(r"\AIHelper\test".to_owned())
    }

    #[test]
    fn an_uninstalled_service_is_not_running_and_has_nothing_to_restore() {
        assert_eq!(state_from_status(&status()), ServiceState::default());
    }

    #[test]
    fn any_one_active_layer_counts_as_running() {
        let ready = Uuid::from_u128(1);
        for (label, mutate) in [
            (
                "readiness",
                Box::new(|status: &mut StatusOutput| {
                    status.readiness.status = ReadinessStatus::Ready;
                    status.readiness.instance_id = Some(ready);
                }) as Box<dyn Fn(&mut StatusOutput)>,
            ),
            (
                "scheduler queued, no runtime yet",
                Box::new(|status: &mut StatusOutput| {
                    status.scheduler.state = SchedulerState::Queued;
                    status.runtime.instance_id = Some(ready);
                }),
            ),
            (
                "runtime starting, readiness silent",
                Box::new(|status: &mut StatusOutput| {
                    status.runtime.status = RuntimeStatus::Starting;
                    status.runtime.instance_id = Some(ready);
                }),
            ),
        ] {
            let mut observed = status();
            mutate(&mut observed);

            assert_eq!(
                state_from_status(&observed),
                ServiceState {
                    was_running: true,
                    previous_instance_id: Some(ready),
                },
                "{label}"
            );
        }
    }

    /// A stale identity from a service that was already down would make
    /// `restore`'s "came back as a different instance" check pass for a service
    /// that never stopped.
    #[test]
    fn an_identity_left_behind_by_a_stopped_service_is_dropped() {
        let mut observed = status();
        observed.readiness.instance_id = Some(Uuid::from_u128(2));
        observed.runtime.instance_id = Some(Uuid::from_u128(3));

        assert_eq!(state_from_status(&observed), ServiceState::default());
    }
}
