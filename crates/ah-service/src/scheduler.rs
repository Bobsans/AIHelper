//! The port between the lifecycle and whatever schedules services on this
//! machine.
//!
//! Everything the lifecycle needs to reason about is named here in neutral
//! terms; everything platform-shaped is `Self::Native`, which only the adapter
//! that produced it ever looks inside. That is what keeps
//! [`crate::lifecycle::run`] free of `cfg`: the unsupported case is an adapter
//! that answers with data ([`UnsupportedScheduler`]) rather than a branch that
//! fails to compile.
//!
//! Drift is on the adapter for the same reason. Comparing a desired
//! [`ServiceSpec`] against an observation would only ever see the properties
//! the neutral model happens to name, and group 06 requires the full
//! platform-level comparison to survive - 27 properties on Windows. So the
//! adapter compares its own projection and reports the result as generic
//! [`DriftEntry`] values.

use ah_error::AppError;

use crate::{
    model::DriftEntry,
    output::SchedulerState,
    spec::{ServiceId, ServiceOwnership, ServiceSpec},
};

/// A service as the scheduler currently holds it.
///
/// `native` is the adapter's own full reading, kept so `drift` can compare
/// every platform property; the fields above it are what the lifecycle reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedService<N> {
    pub id: ServiceId,
    pub marker: crate::model::TaskMarker,
    pub state: SchedulerState,
    pub last_result: Option<i32>,
    pub last_run_at: Option<String>,
    /// Whether the scheduler will leave restarts to AIHelper. A scheduler that
    /// retries on its own races the lifecycle for the instance lease, so the
    /// status reduction has to know.
    pub canonical_restart_policy: bool,
    pub native: N,
}

impl<N> ObservedService<N> {
    pub fn ownership(&self) -> ServiceOwnership {
        ServiceOwnership {
            id: self.id.clone(),
            marker: self.marker.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceObservation<N> {
    Missing,
    Owned(Box<ObservedService<N>>),
    /// Something occupies the identity but does not carry our markers.
    Foreign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerRunReceipt {
    pub submitted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerInstance {
    pub instance_id: uuid::Uuid,
    pub state: SchedulerState,
    pub engine_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerStopTarget {
    Running {
        instance_id: uuid::Uuid,
        expected_pid: u32,
    },
    Queued {
        instance_id: uuid::Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerStopReceipt {
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerDeleteReceipt {
    pub deleted: bool,
}

pub trait ServiceScheduler {
    /// The adapter's own full reading of a registration. Opaque to the
    /// lifecycle; only `drift` interprets it.
    type Native: Clone + std::fmt::Debug + PartialEq;

    /// Whether this adapter can manage a service at all. Only
    /// [`UnsupportedScheduler`] says no, so a caller that wants to refuse early
    /// with its own diagnostic - `ah ai install --transport managed` does -
    /// asks the adapter rather than testing the platform itself.
    const SUPPORTED: bool = true;

    /// Who this machine says we are, and where this scheduler would keep the
    /// service for that account.
    fn identity(&self) -> Result<ServiceId, AppError>;

    fn inspect(&self, id: &ServiceId) -> Result<ServiceObservation<Self::Native>, AppError>;

    fn register(&self, desired: &ServiceSpec) -> Result<ObservedService<Self::Native>, AppError>;

    fn run(&self, id: &ServiceId) -> Result<SchedulerRunReceipt, AppError>;

    fn instances(&self, expected: &ServiceOwnership) -> Result<Vec<SchedulerInstance>, AppError>;

    fn stop_instance(
        &self,
        expected: &ServiceSpec,
        target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError>;

    fn delete_owned(&self, expected: &ServiceOwnership)
    -> Result<SchedulerDeleteReceipt, AppError>;

    /// Every platform property of `observed` that no longer matches `desired`.
    fn drift(
        &self,
        desired: &ServiceSpec,
        observed: &ObservedService<Self::Native>,
    ) -> Vec<DriftEntry>;
}

/// This platform's scheduler.
#[cfg(windows)]
pub type PlatformScheduler = crate::windows_scheduler::WindowsTaskScheduler;

#[cfg(target_os = "linux")]
pub type PlatformScheduler = crate::systemd_scheduler::SystemdUserScheduler;

#[cfg(not(any(windows, target_os = "linux")))]
pub type PlatformScheduler = UnsupportedScheduler;

pub fn platform_scheduler() -> PlatformScheduler {
    PlatformScheduler::default()
}

/// Refuse a persisted registration identity this platform's scheduler would
/// never have produced.
///
/// The shape is the scheduler's own - a rooted path in the Task Scheduler's
/// namespace, a bare unit name for systemd - and what both have to refuse is
/// the same thing: a value that would resolve somewhere other than where the
/// adapter puts its registrations. The identity is also compared against the
/// live one before use, so this is the outer of two checks rather than the
/// only one.
///
/// # Errors
///
/// [`AppError`] with `MCP_SERVICE_STATE_INVALID` when the identity could name
/// something else.
pub fn validate_registration_identity(identity: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        if !identity.starts_with('\\') {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "task_path must be absolute",
            ));
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let component = !identity.is_empty()
            && identity != "."
            && identity != ".."
            && !identity.contains('/')
            && !identity.contains('\\');
        if !component {
            return Err(AppError::external(
                "MCP_SERVICE_STATE_INVALID",
                "task_path must be a single unit name",
            ));
        }
        Ok(())
    }
}

/// The scheduler of a platform AIHelper does not manage services on yet.
///
/// It exists so that call sites stop being `cfg`-gated: an unsupported
/// platform is an adapter that refuses every operation with the same
/// diagnostic, not a second copy of the lifecycle that does not compile.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedScheduler;

impl UnsupportedScheduler {
    fn refuse<T>() -> Result<T, AppError> {
        Err(unsupported_platform())
    }
}

pub fn unsupported_platform() -> AppError {
    AppError::external(
        "MCP_SERVICE_UNSUPPORTED_PLATFORM",
        "managed MCP service lifecycle is not supported on this platform",
    )
}

impl ServiceScheduler for UnsupportedScheduler {
    type Native = ();

    const SUPPORTED: bool = false;

    fn identity(&self) -> Result<ServiceId, AppError> {
        Self::refuse()
    }

    fn inspect(&self, _id: &ServiceId) -> Result<ServiceObservation<()>, AppError> {
        Self::refuse()
    }

    fn register(&self, _desired: &ServiceSpec) -> Result<ObservedService<()>, AppError> {
        Self::refuse()
    }

    fn run(&self, _id: &ServiceId) -> Result<SchedulerRunReceipt, AppError> {
        Self::refuse()
    }

    fn instances(&self, _expected: &ServiceOwnership) -> Result<Vec<SchedulerInstance>, AppError> {
        Self::refuse()
    }

    fn stop_instance(
        &self,
        _expected: &ServiceSpec,
        _target: &SchedulerStopTarget,
    ) -> Result<SchedulerStopReceipt, AppError> {
        Self::refuse()
    }

    fn delete_owned(
        &self,
        _expected: &ServiceOwnership,
    ) -> Result<SchedulerDeleteReceipt, AppError> {
        Self::refuse()
    }

    fn drift(&self, _desired: &ServiceSpec, _observed: &ObservedService<()>) -> Vec<DriftEntry> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every refusal carries the same code, because the lifecycle reports it
    /// verbatim and the code is part of the released error table.
    #[test]
    fn an_unsupported_platform_refuses_with_one_diagnostic() {
        let scheduler = UnsupportedScheduler;
        let id = ServiceId {
            owner: "owner".to_owned(),
            path: "path".to_owned(),
        };
        for error in [
            scheduler.identity().err(),
            scheduler.inspect(&id).err(),
            scheduler.run(&id).err(),
        ] {
            let error = error.expect("an unsupported scheduler refuses");
            assert_eq!(error.code(), "MCP_SERVICE_UNSUPPORTED_PLATFORM");
        }
    }
}
