use serde::Serialize;

use crate::{
    error::AppError,
    mcp_service::output::{ReadinessStatus, RegistrationStatus, StatusOutput},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedState {
    Running,
    Stopped,
    NotInstalled,
    NeedsRepair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub state: ManagedState,
    pub endpoint: Option<String>,
    pub diagnostic_code: Option<String>,
}

impl Snapshot {
    /// Label used by the interactive transport menu.
    pub fn label(&self) -> String {
        let endpoint = self
            .endpoint
            .clone()
            .unwrap_or_else(|| super::install::DEFAULT_HTTP_URL.to_owned());
        match self.state {
            ManagedState::Running => format!("Managed service ({endpoint}) — running"),
            ManagedState::Stopped => {
                format!("Managed service ({endpoint}) — installed, stopped; will be started")
            }
            ManagedState::NotInstalled => {
                format!("Managed service ({endpoint}) — will be installed")
            }
            ManagedState::NeedsRepair => {
                let code = self.diagnostic_code.as_deref().unwrap_or("unknown");
                format!("Managed service — needs repair ({code})")
            }
        }
    }
}

pub fn is_supported() -> bool {
    cfg!(windows)
}

fn unsupported() -> AppError {
    AppError::external(
        "AI_MANAGED_UNSUPPORTED",
        "the managed MCP service is available only on Windows; \
         use --transport http against a running server, or --transport stdio",
    )
}

/// Classify a managed-service status snapshot. Kept free of platform gates so
/// the mapping can be tested everywhere.
pub fn classify(status: &StatusOutput) -> Snapshot {
    let endpoint = status.runtime.endpoint.clone();
    match status.registration.status {
        RegistrationStatus::ConfigurationDrift | RegistrationStatus::SchedulerError => Snapshot {
            state: ManagedState::NeedsRepair,
            endpoint,
            diagnostic_code: status
                .registration
                .diagnostic_code
                .clone()
                .or_else(|| status.scheduler.diagnostic_code.clone())
                .or_else(|| status.runtime.diagnostic_code.clone()),
        },
        RegistrationStatus::NotInstalled => Snapshot {
            state: ManagedState::NotInstalled,
            endpoint: None,
            diagnostic_code: Some("MCP_SERVICE_NOT_INSTALLED".to_owned()),
        },
        RegistrationStatus::Installed => Snapshot {
            state: if status.readiness.status == ReadinessStatus::Ready {
                ManagedState::Running
            } else {
                ManagedState::Stopped
            },
            endpoint,
            diagnostic_code: None,
        },
    }
}

#[cfg(windows)]
pub fn detect() -> Result<Snapshot, AppError> {
    Ok(classify(&crate::mcp_service::lifecycle::snapshot_status()?))
}

#[cfg(not(windows))]
pub fn detect() -> Result<Snapshot, AppError> {
    Err(unsupported())
}

/// What `ensure_ready` had to do to reach a usable endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAction {
    AlreadyRunning,
    Started,
    Installed,
}

impl ManagedAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlreadyRunning => "already_running",
            Self::Started => "started",
            Self::Installed => "installed",
        }
    }
}

fn not_healthy(snapshot: &Snapshot) -> AppError {
    let code = snapshot.diagnostic_code.as_deref().unwrap_or("unknown");
    AppError::external(
        "AI_MANAGED_NOT_HEALTHY",
        format!(
            "the managed MCP service needs repair ({code}); \
             inspect it with `ah mcp service status` before wiring an agent to it"
        ),
    )
}

fn missing_endpoint() -> AppError {
    AppError::external(
        "AI_MANAGED_ENDPOINT_UNKNOWN",
        "the managed MCP service did not report an endpoint; \
         inspect it with `ah mcp service status`",
    )
}

/// Reject a snapshot that must not be wired to an agent, before any mutation
/// and regardless of `--dry-run`.
pub fn require_usable(snapshot: &Snapshot) -> Result<(), AppError> {
    if !is_supported() {
        return Err(unsupported());
    }
    if snapshot.state == ManagedState::NeedsRepair {
        return Err(not_healthy(snapshot));
    }
    Ok(())
}

/// The action `ensure_ready` would take, without taking it.
pub fn planned_action(snapshot: &Snapshot) -> ManagedAction {
    match snapshot.state {
        ManagedState::Running => ManagedAction::AlreadyRunning,
        ManagedState::Stopped => ManagedAction::Started,
        ManagedState::NotInstalled | ManagedState::NeedsRepair => ManagedAction::Installed,
    }
}

#[cfg(windows)]
pub fn ensure_ready(snapshot: &Snapshot) -> Result<(String, ManagedAction), AppError> {
    use crate::{
        cli::GlobalOptions,
        mcp_service::{
            command::InstallOptions,
            model::{DEFAULT_MAX_ACTIVE, DEFAULT_PORT, DEFAULT_TIMEOUT_MS},
        },
        output::OutputMode,
    };

    require_usable(snapshot)?;
    match snapshot.state {
        ManagedState::Running => Ok((
            snapshot.endpoint.clone().ok_or_else(missing_endpoint)?,
            ManagedAction::AlreadyRunning,
        )),
        ManagedState::Stopped => {
            let output = crate::mcp_service::lifecycle::start_quietly()?;
            Ok((output.endpoint, ManagedAction::Started))
        }
        ManagedState::NotInstalled => {
            let options = InstallOptions {
                no_start: false,
                port: DEFAULT_PORT,
                max_active: DEFAULT_MAX_ACTIVE,
                default_timeout_ms: DEFAULT_TIMEOUT_MS,
                options: GlobalOptions {
                    output: OutputMode::Text,
                    quiet: true,
                    limit: None,
                },
            };
            let output = crate::mcp_service::lifecycle::install_quietly(&options)?;
            Ok((output.endpoint, ManagedAction::Installed))
        }
        ManagedState::NeedsRepair => Err(not_healthy(snapshot)),
    }
}

#[cfg(not(windows))]
pub fn ensure_ready(snapshot: &Snapshot) -> Result<(String, ManagedAction), AppError> {
    let _ = snapshot;
    Err(unsupported())
}

#[cfg(test)]
mod tests {
    use super::{ManagedAction, ManagedState, classify, planned_action, require_usable};
    use crate::mcp_service::output::{ReadinessStatus, RegistrationStatus, StatusOutput};

    fn snapshot_of(
        registration: RegistrationStatus,
        readiness: ReadinessStatus,
        endpoint: Option<&str>,
    ) -> StatusOutput {
        let mut status = StatusOutput::not_installed("\\AIHelper Managed MCP".to_owned());
        status.registration.status = registration;
        status.registration.diagnostic_code = match registration {
            RegistrationStatus::ConfigurationDrift => {
                Some("MCP_SERVICE_CONFIGURATION_DRIFT".to_owned())
            }
            RegistrationStatus::SchedulerError => Some("MCP_SERVICE_SCHEDULER_FAILED".to_owned()),
            _ => None,
        };
        status.readiness.status = readiness;
        status.runtime.endpoint = endpoint.map(str::to_owned);
        status
    }

    #[test]
    fn a_ready_installation_is_running_and_keeps_its_own_endpoint() {
        let snapshot = classify(&snapshot_of(
            RegistrationStatus::Installed,
            ReadinessStatus::Ready,
            Some("http://127.0.0.1:9000/mcp"),
        ));
        assert_eq!(snapshot.state, ManagedState::Running);
        assert_eq!(
            snapshot.endpoint.as_deref(),
            Some("http://127.0.0.1:9000/mcp"),
            "a service on a non-default port must not be reported as 8787"
        );
        assert_eq!(planned_action(&snapshot), ManagedAction::AlreadyRunning);
    }

    #[test]
    fn an_installed_but_unready_service_is_stopped() {
        let snapshot = classify(&snapshot_of(
            RegistrationStatus::Installed,
            ReadinessStatus::NotReady,
            Some("http://127.0.0.1:8787/mcp"),
        ));
        assert_eq!(snapshot.state, ManagedState::Stopped);
        assert_eq!(planned_action(&snapshot), ManagedAction::Started);
    }

    #[test]
    fn a_missing_registration_is_reported_as_not_installed() {
        let snapshot = classify(&snapshot_of(
            RegistrationStatus::NotInstalled,
            ReadinessStatus::NotChecked,
            None,
        ));
        assert_eq!(snapshot.state, ManagedState::NotInstalled);
        assert_eq!(planned_action(&snapshot), ManagedAction::Installed);
        assert!(snapshot.label().contains("will be installed"));
    }

    #[test]
    fn drift_and_scheduler_errors_are_refused_rather_than_repaired() {
        for registration in [
            RegistrationStatus::ConfigurationDrift,
            RegistrationStatus::SchedulerError,
        ] {
            let snapshot = classify(&snapshot_of(
                registration,
                ReadinessStatus::Error,
                Some("http://127.0.0.1:8787/mcp"),
            ));
            assert_eq!(snapshot.state, ManagedState::NeedsRepair);
            assert!(snapshot.diagnostic_code.is_some());
            let error = require_usable(&snapshot).expect_err("a drifted service must be refused");
            let expected = if cfg!(windows) {
                "AI_MANAGED_NOT_HEALTHY"
            } else {
                "AI_MANAGED_UNSUPPORTED"
            };
            assert_eq!(error.code(), expected);
            assert!(snapshot.label().contains("needs repair"));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn a_healthy_snapshot_is_still_refused_off_windows() {
        let snapshot = classify(&snapshot_of(
            RegistrationStatus::Installed,
            ReadinessStatus::Ready,
            Some("http://127.0.0.1:8787/mcp"),
        ));
        let error = require_usable(&snapshot).expect_err("managed is Windows-only");
        assert_eq!(error.code(), "AI_MANAGED_UNSUPPORTED");
    }
}
