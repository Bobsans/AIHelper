//! What the updater needs from a managed service it is about to interrupt.
//!
//! The updater used to call `mcp_service::lifecycle` directly, which pointed
//! the dependency the wrong way: the subsystem that must not care whether a
//! managed service exists knew six of its functions by name. It depends on this
//! trait instead, `mcp_service` implements it, and `runtime_flow` supplies the
//! implementation.
//!
//! Every method other than [`ServiceGuard::hold`] takes the hold as a
//! parameter. That is the point of the parameter: these operations are only
//! sound while the lifecycle lease is held, and the three functions this
//! replaced said so in their names (`*_while_locked`) where nothing could check
//! it.

use std::time::Duration;

use uuid::Uuid;

use crate::error::AppError;

/// The lifecycle lease, held for as long as the update needs the service to
/// stay where it was put.
///
/// Concretely a process lease, because the update helper inherits its handle
/// and needs the real thing. It is `ah_platform`'s rather than
/// `mcp_service`'s - the updater must not know which subsystem the lease
/// belongs to, only that it holds one.
pub(crate) type ServiceHold = ah_platform::lease::FileLease;

/// What the service was, so it can be put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ServiceState {
    pub(crate) was_running: bool,
    /// The instance that was serving, so a restore can prove it came back as a
    /// *different* one rather than never having stopped.
    pub(crate) previous_instance_id: Option<Uuid>,
}

/// A managed service the updater has to hold still while it swaps the binary
/// under it.
pub(crate) trait ServiceGuard {
    /// Take the lifecycle lease: until the returned hold is dropped, nothing
    /// else may change the service's lifecycle.
    ///
    /// # Errors
    ///
    /// [`AppError`] when the lease is held elsewhere for longer than `timeout`,
    /// or when the service's own paths cannot be resolved.
    fn hold(&self, timeout: Duration) -> Result<ServiceHold, AppError>;

    /// What the service is now, in the terms a later [`Self::restore`] needs.
    ///
    /// # Errors
    ///
    /// [`AppError`] when the service's state cannot be read.
    fn capture(&self, hold: &ServiceHold) -> Result<ServiceState, AppError>;

    /// Stop it, reporting whether it had been running.
    ///
    /// # Errors
    ///
    /// [`AppError`] when it cannot be stopped, or cannot be proved stopped.
    fn stop(&self, hold: &ServiceHold) -> Result<bool, AppError>;

    /// Put it back the way [`Self::capture`] found it.
    ///
    /// A state that was not running restores to nothing.
    ///
    /// # Errors
    ///
    /// [`AppError`] when it does not come back, or comes back as the same
    /// instance that was supposed to have stopped.
    fn restore(&self, hold: &ServiceHold, state: ServiceState) -> Result<(), AppError>;
}
