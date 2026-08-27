//! What the managed service *is*, in terms no operating system owns.
//!
//! The scheduler port ([`crate::scheduler`]) takes one of these and makes it
//! true; a platform adapter projects it onto whatever its own scheduler
//! understands - a Task Scheduler definition, a systemd unit, a launchd plist.
//! Nothing here names a Windows concept, which is the point: the 27-field
//! Task Scheduler spec that used to sit in this position could only be
//! implemented by Windows, so the generic seam below it bought testability
//! and not portability.

use std::{path::PathBuf, time::Duration};

use crate::model::TaskMarker;

/// Whose service it is, and where the platform's scheduler keeps it.
///
/// `owner` is the account the service runs as, spelled the way that platform
/// spells accounts (a SID on Windows). `path` is the scheduler's own name for
/// the registration, derived from `owner` by the adapter - the two travel
/// together because every lookup needs both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceId {
    pub owner: String,
    pub path: String,
}

/// The desired state of one managed service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub id: ServiceId,
    /// The ownership stamp the adapter writes alongside the registration, so a
    /// later read can prove the registration is still ours.
    pub marker: TaskMarker,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub start: StartPolicy,
    pub restart: RestartPolicy,
    pub resource: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPolicy {
    /// Start when the owning account logs on, and on demand.
    AtLogon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// The scheduler never restarts the service on its own. AIHelper's own
    /// lifecycle owns restarts, and a scheduler that also retried would race
    /// it for the instance lease.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// `None` means the service may run for as long as it likes.
    pub execution_time: Option<Duration>,
    /// Whether a second start request while one instance runs is refused.
    pub single_instance: bool,
}

/// The subset of a spec that proves a registration is still the one we wrote.
///
/// Deleting and enumerating do not need the desired state, only the identity
/// to check it against - and `uninstall` has to be able to build one from an
/// observation, when the definition that produced the spec is already gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceOwnership {
    pub id: ServiceId,
    pub marker: TaskMarker,
}

impl ServiceSpec {
    /// The one spec AIHelper installs: the managed MCP server, started at
    /// logon, restarted by nobody but us, single-instance, unbounded.
    pub fn managed_mcp(
        id: ServiceId,
        marker: TaskMarker,
        executable: PathBuf,
        working_directory: PathBuf,
    ) -> Self {
        let arguments = vec![
            "mcp".to_owned(),
            "serve".to_owned(),
            "--transport".to_owned(),
            "http".to_owned(),
            "--managed-config".to_owned(),
            marker.definition_path.to_string_lossy().into_owned(),
        ];
        Self {
            id,
            marker,
            executable,
            arguments,
            working_directory,
            start: StartPolicy::AtLogon,
            restart: RestartPolicy::Never,
            resource: ResourceLimits {
                execution_time: None,
                single_instance: true,
            },
        }
    }

    pub fn ownership(&self) -> ServiceOwnership {
        ServiceOwnership {
            id: self.id.clone(),
            marker: self.marker.clone(),
        }
    }
}
