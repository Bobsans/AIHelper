//! The managed service's lifecycle and instance leases, in this CLI's wording.
//!
//! The lease mechanism is `ah_platform::lease`, which reports `io::Error` and
//! knows nothing about a managed MCP service. This module is the layer that
//! gives those failures the three error codes the service has always reported,
//! and that supplies the mutex name - derived by the rule `ah` and the update
//! helper both have to agree on.

use std::{path::Path, time::Duration};

use ah_error::AppError;

pub use ah_platform::lease::FileLease;

/// Take the lease if it is free, reporting `Ok(None)` if the service holds it.
///
/// # Errors
///
/// [`AppError`] with `MCP_SERVICE_UNSUPPORTED_PLATFORM` off Windows, or
/// `MCP_SERVICE_STATE_INVALID` when the OS refuses to name the lease.
pub fn try_acquire(path: &Path) -> Result<Option<FileLease>, AppError> {
    FileLease::try_acquire(path, &ah_updater_core::lifecycle_mutex_name(path))
        .map_err(|error| describe(path, error))
}

/// Take the lease, waiting up to `timeout` for whoever holds it.
///
/// # Errors
///
/// [`AppError`] with `MCP_SERVICE_BUSY` when it is still held at the deadline;
/// otherwise as [`try_acquire`].
pub fn acquire(path: &Path, timeout: Duration) -> Result<FileLease, AppError> {
    FileLease::acquire(path, &ah_updater_core::lifecycle_mutex_name(path), timeout)
        .map_err(|error| describe(path, error))
}

/// The three codes this has always reported, kept exactly.
fn describe(path: &Path, error: std::io::Error) -> AppError {
    match error.kind() {
        std::io::ErrorKind::Unsupported => AppError::external(
            "MCP_SERVICE_UNSUPPORTED_PLATFORM",
            "managed MCP service lifecycle is supported only on Windows",
        ),
        std::io::ErrorKind::TimedOut => AppError::external(
            "MCP_SERVICE_BUSY",
            format!(
                "timed out waiting for managed MCP lifecycle lease '{}'",
                path.display()
            ),
        ),
        _ => AppError::external(
            "MCP_SERVICE_STATE_INVALID",
            format!(
                "failed to create managed MCP mutex '{}': {error}",
                path.display()
            ),
        ),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_busy_lease_reports_the_stable_diagnostic() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("lifecycle.lock");
        let _held = try_acquire(&path).unwrap().unwrap();

        let error = acquire(&path, Duration::from_millis(1)).unwrap_err();

        assert_eq!(error.code(), "MCP_SERVICE_BUSY");
    }
}
