//! What the server is configured with, what it reports, and the tool-name and
//! metadata keys that are part of its wire contract.

use super::*;

pub(crate) const TOOL_PREFIX: &str = "ah.";

pub(crate) const RISK_META_KEY: &str = "dev.aihelper/risk";

pub(crate) const DIAGNOSTIC_META_KEY: &str = "dev.aihelper/diagnostic";

pub(crate) const EXECUTION_META_KEY: &str = "dev.aihelper/execution";

pub(crate) const JOB_TOOL_PREFIX: &str = "ah.job.";

pub(crate) const JOB_START_TOOL: &str = "ah.job.start";

pub(crate) const JOB_STATUS_TOOL: &str = "ah.job.status";

pub(crate) const JOB_RESULT_TOOL: &str = "ah.job.result";

pub(crate) const JOB_CANCEL_TOOL: &str = "ah.job.cancel";

pub(crate) const PEER_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCommandStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpCommandEvent {
    pub command: String,
    pub tool: String,
    pub request_id: String,
    pub job_id: Option<String>,
    pub parameters: Value,
    pub status: McpCommandStatus,
    pub duration_ms: u64,
    pub diagnostic: Option<CommandError>,
    pub outcome: Option<InvocationOutcome>,
}

pub trait EventSink: Send + Sync {
    fn record_command(&self, event: McpCommandEvent);

    fn record_command_with_telemetry(
        &self,
        event: McpCommandEvent,
        _telemetry: Option<ExecutionTelemetry>,
    ) {
        self.record_command(event);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerConfig {
    pub cwd: String,
    pub limit: Option<usize>,
    pub default_timeout_ms: u64,
}

impl McpServerConfig {
    pub fn new(
        cwd: impl Into<String>,
        limit: Option<usize>,
        default_timeout_ms: u64,
    ) -> Result<Self, McpAdapterError> {
        let cwd = cwd.into();
        if cwd.trim().is_empty() {
            return Err(McpAdapterError::InvalidConfig(
                "default cwd must not be empty".to_owned(),
            ));
        }
        if limit == Some(0) {
            return Err(McpAdapterError::InvalidConfig(
                "default limit must be greater than zero".to_owned(),
            ));
        }
        if default_timeout_ms == 0 {
            return Err(McpAdapterError::InvalidConfig(
                "default timeout must be greater than zero".to_owned(),
            ));
        }
        Ok(Self {
            cwd,
            limit,
            default_timeout_ms,
        })
    }
}

#[derive(Debug, Error)]
pub enum McpAdapterError {
    #[error("invalid MCP server configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid typed command schema for '{command}': {reason}")]
    InvalidSchema { command: String, reason: String },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("MCP service failed: {0}")]
    Service(String),
    #[error("MCP shutdown exceeded the configured {grace_ms} ms grace period")]
    ShutdownTimeout { grace_ms: u128 },
}

#[must_use]
pub struct McpServeOutcome {
    pub(crate) result: Result<(), McpAdapterError>,
    pub(crate) remaining_shutdown_grace: Duration,
}

impl McpServeOutcome {
    pub fn into_parts(self) -> (Result<(), McpAdapterError>, Duration) {
        (self.result, self.remaining_shutdown_grace)
    }

    pub fn into_result(self) -> Result<(), McpAdapterError> {
        self.result
    }
}
