//! The runtime's error type, and the diagnostic it renders into.
//!
//! One `From<RuntimeError>` per consumer, so no caller maps codes by hand.

use super::*;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("plugin not found for domain '{0}'")]
    DomainNotFound(String),
    #[error("failed to load plugin library at {path:?}: {source}")]
    LibraryLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("failed to load plugin entrypoint from {path:?}: {source}")]
    SymbolLoad {
        path: PathBuf,
        source: libloading::Error,
    },
    #[error("plugin at {path:?} has incompatible abi version {found}, expected {expected}")]
    AbiVersionMismatch {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error(
        "plugin at {path:?} requires unsupported plugin api version {found_major}.{found_minor}, host supports {supported_major}.{supported_minor}"
    )]
    ApiVersionMismatch {
        path: PathBuf,
        found_major: u16,
        found_minor: u16,
        supported_major: u16,
        supported_minor: u16,
    },
    #[error("plugin at {path:?} returned invalid metadata: {reason}")]
    InvalidMetadata { path: PathBuf, reason: String },
    #[error("plugin invocation failed: {0}")]
    Invocation(String),
    #[error("plugin response parse failed: {0}")]
    ResponseParse(String),
    #[error("invalid typed command catalog for domain '{domain}': {reason}")]
    InvalidCommandCatalog { domain: String, reason: String },
    #[error("typed command not found: {0}")]
    TypedCommandNotFound(String),
    #[error("typed command invocation failed: {0}")]
    TypedInvocation(String),
    #[error("required credential slot '{slot}' is missing for '{command}'")]
    SecretRequired { command: String, slot: String },
    #[error("credential '{id}' for slot '{slot}' was not found for '{command}'")]
    SecretNotFound {
        command: String,
        slot: String,
        id: String,
    },
    #[error(
        "credential '{id}' for slot '{slot}' has kind '{kind}', expected one of {accepted_kinds:?} for '{command}'"
    )]
    SecretKindMismatch {
        command: String,
        slot: String,
        id: String,
        kind: String,
        accepted_kinds: Vec<String>,
    },
    #[error("vault is locked while resolving credential '{id}' for slot '{slot}' in '{command}'")]
    VaultLocked {
        command: String,
        slot: String,
        id: String,
    },
    #[error(
        "vault key is unavailable while resolving credential '{id}' for slot '{slot}' in '{command}'"
    )]
    VaultKeyUnavailable {
        command: String,
        slot: String,
        id: String,
    },
    #[error("typed command response failed validation for '{command}': {reason}")]
    TypedResponseValidation { command: String, reason: String },
    #[error("typed execution request is invalid: {0}")]
    InvalidExecutionRequest(String),
    #[error("typed execution capacity is full (maximum active {capacity})")]
    ExecutionCapacityFull { capacity: usize },
    #[error("typed execution request '{request_id}' was cancelled")]
    ExecutionCancelled { request_id: String },
    #[error("typed execution request '{request_id}' timed out")]
    ExecutionTimeout { request_id: String },
    #[error("typed executor is shutting down")]
    ExecutorShuttingDown,
    #[error("typed execution worker failed: {0}")]
    ExecutionWorker(String),
    #[error("typed execution handler panicked for request '{request_id}'")]
    ExecutionPanic { request_id: String },
    #[error("plugin domain '{0}' is disabled")]
    DomainDisabled(String),
    #[error("required external tool '{tool}' is not available for domain '{domain}'")]
    DependencyMissing {
        domain: String,
        operation: Option<String>,
        tool: String,
        reason: String,
    },
}

/// Per-variant error facts, produced by the single exhaustive match over
/// `RuntimeError` in [`RuntimeError::describe`].
///
/// `diagnostic` is the diagnostic currency every surface consumes. The
/// `agent_*` fields record the handful of places where the frozen MCP wire
/// contract disagrees with the host contract, and `retryable` is meaningful
/// only to MCP clients — the CLI never retries a failed command.
pub(super) struct RuntimeDiagnostic {
    pub(super) diagnostic: ErrorDiagnostic,
    pub(super) agent_code: Option<&'static str>,
    pub(super) agent_exit_code_hint: i32,
    pub(super) retryable: bool,
}

impl RuntimeDiagnostic {
    pub(super) fn new(code: &'static str, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            diagnostic: ErrorDiagnostic::new(None, None, code, message.clone(), message, 1),
            agent_code: None,
            agent_exit_code_hint: 1,
            retryable: false,
        }
    }

    pub(super) fn cause(mut self, cause: impl Into<String>) -> Self {
        self.diagnostic.cause = cause.into();
        self
    }

    pub(super) fn domain(mut self, domain: Option<String>) -> Self {
        self.diagnostic.domain = domain;
        self
    }

    pub(super) fn operation(mut self, operation: Option<String>) -> Self {
        self.diagnostic.operation = operation;
        self
    }

    pub(super) fn agent_code(mut self, code: &'static str) -> Self {
        self.agent_code = Some(code);
        self
    }

    pub(super) fn agent_exit_code_hint(mut self, hint: i32) -> Self {
        self.agent_exit_code_hint = hint;
        self
    }

    pub(super) fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }
}

pub(super) fn command_domain(command: &str) -> Option<String> {
    command.split_once('.').map(|(domain, _)| domain.to_owned())
}

impl RuntimeError {
    /// The one translation table from `RuntimeError` to the diagnostic
    /// currency. Every surface — CLI, MCP, event log — projects from here, so
    /// adding a variant breaks exactly this match.
    ///
    /// Credential identifiers, credential kinds and accepted-kind lists are
    /// deliberately dropped from the secret and vault arms: `Display` may name
    /// them for local debugging, but a diagnostic is rendered to users, written
    /// to the event log and handed to MCP clients, so it never carries them.
    pub(super) fn describe(&self) -> RuntimeDiagnostic {
        match self {
            Self::DomainNotFound(domain) => RuntimeDiagnostic::new(
                "DOMAIN_NOT_FOUND",
                format!("unknown command domain: {domain}"),
            )
            .domain(Some(domain.clone()))
            .agent_exit_code_hint(2),
            Self::LibraryLoad { path, source } => RuntimeDiagnostic::new(
                "PLUGIN_LIBRARY_LOAD_FAILED",
                format!(
                    "failed to load plugin library '{}': {source}",
                    path.display()
                ),
            ),
            Self::SymbolLoad { path, source } => RuntimeDiagnostic::new(
                "PLUGIN_SYMBOL_LOAD_FAILED",
                format!(
                    "failed to load plugin entrypoint '{}': {source}",
                    path.display()
                ),
            ),
            Self::AbiVersionMismatch {
                path,
                found,
                expected,
            } => RuntimeDiagnostic::new(
                "PLUGIN_ABI_MISMATCH",
                format!(
                    "plugin '{}' has incompatible ABI version {found}; expected {expected}",
                    path.display()
                ),
            ),
            Self::ApiVersionMismatch {
                path,
                found_major,
                found_minor,
                supported_major,
                supported_minor,
            } => RuntimeDiagnostic::new(
                "PLUGIN_API_MISMATCH",
                format!(
                    "plugin '{}' requires unsupported Plugin API version {found_major}.{found_minor}; host supports {supported_major}.{supported_minor}",
                    path.display()
                ),
            ),
            Self::InvalidMetadata { path, reason } => RuntimeDiagnostic::new(
                "PLUGIN_METADATA_INVALID",
                format!(
                    "plugin '{}' returned invalid metadata: {reason}",
                    path.display()
                ),
            ),
            Self::Invocation(message) => {
                RuntimeDiagnostic::new("PLUGIN_INVOCATION_FAILED", message.clone())
            }
            Self::ResponseParse(message) => {
                RuntimeDiagnostic::new("PLUGIN_RESPONSE_PARSE_FAILED", message.clone())
                    .agent_code("PLUGIN_RESPONSE_INVALID")
            }
            Self::InvalidCommandCatalog { domain, reason } => RuntimeDiagnostic::new(
                "COMMAND_CATALOG_INVALID",
                format!("invalid typed command catalog for domain '{domain}': {reason}"),
            ),
            Self::TypedCommandNotFound(command) => RuntimeDiagnostic::new(
                "TYPED_COMMAND_NOT_FOUND",
                format!("typed command not found: {command}"),
            )
            .domain(command_domain(command))
            .operation(Some(command.clone()))
            .agent_code("COMMAND_NOT_FOUND")
            .agent_exit_code_hint(2)
            .retryable(),
            Self::TypedInvocation(message) => {
                RuntimeDiagnostic::new("TYPED_INVOCATION_FAILED", message.clone())
            }
            Self::SecretRequired { command, slot } => RuntimeDiagnostic::new(
                "SECRET_REQUIRED",
                format!("required credential slot '{slot}' is missing for '{command}'"),
            ),
            Self::SecretNotFound { command, slot, .. } => RuntimeDiagnostic::new(
                "SECRET_NOT_FOUND",
                format!("credential for slot '{slot}' was not found for '{command}'"),
            ),
            Self::SecretKindMismatch { .. } => RuntimeDiagnostic::new(
                "SECRET_KIND_MISMATCH",
                "credential kind does not match the required credential slot",
            ),
            Self::VaultLocked { command, slot, .. } => RuntimeDiagnostic::new(
                "VAULT_LOCKED",
                format!(
                    "vault is locked while resolving credential for slot '{slot}' in '{command}'"
                ),
            ),
            Self::VaultKeyUnavailable { command, slot, .. } => RuntimeDiagnostic::new(
                "VAULT_KEY_UNAVAILABLE",
                format!(
                    "vault key is unavailable while resolving credential for slot '{slot}' in '{command}'"
                ),
            ),
            Self::TypedResponseValidation { command, reason } => RuntimeDiagnostic::new(
                "OUTPUT_SCHEMA_VIOLATION",
                format!("typed command response failed validation for '{command}': {reason}"),
            ),
            Self::InvalidExecutionRequest(message) => {
                RuntimeDiagnostic::new("EXECUTION_REQUEST_INVALID", message.clone())
            }
            Self::ExecutionCapacityFull { capacity } => RuntimeDiagnostic::new(
                "EXECUTION_CAPACITY_FULL",
                format!("typed execution capacity is full (maximum active {capacity})"),
            )
            .retryable(),
            Self::ExecutionCancelled { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_CANCELLED",
                format!("typed execution request '{request_id}' was cancelled"),
            )
            .agent_code("CANCELLED"),
            Self::ExecutionTimeout { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_TIMEOUT",
                format!("typed execution request '{request_id}' timed out"),
            )
            .agent_code("TIMEOUT")
            .retryable(),
            Self::ExecutorShuttingDown => {
                RuntimeDiagnostic::new("EXECUTOR_SHUTTING_DOWN", "typed executor is shutting down")
            }
            Self::ExecutionWorker(message) => {
                RuntimeDiagnostic::new("EXECUTION_WORKER_FAILED", message.clone())
            }
            Self::ExecutionPanic { request_id } => RuntimeDiagnostic::new(
                "EXECUTION_HANDLER_PANIC",
                format!("typed execution handler panicked for request '{request_id}'"),
            )
            .agent_code("HANDLER_PANIC"),
            Self::DomainDisabled(domain) => RuntimeDiagnostic::new(
                "DOMAIN_DISABLED",
                format!("plugin domain is disabled: {domain}"),
            )
            .domain(Some(domain.clone()))
            .agent_exit_code_hint(2),
            Self::DependencyMissing {
                domain,
                operation,
                tool,
                reason,
            } => RuntimeDiagnostic::new(
                "DEPENDENCY_MISSING",
                format!("required external tool not found: {tool}"),
            )
            .cause(reason.clone())
            .domain(Some(domain.clone()))
            .operation(operation.clone()),
        }
    }

    /// Host-facing diagnostic: the currency `AppError` and the event log speak.
    pub fn diagnostic(&self) -> ErrorDiagnostic {
        self.describe().diagnostic
    }

    /// MCP-facing error: the same facts plus the retryability an agent needs,
    /// under the code strings the MCP wire contract froze.
    pub fn command_error(&self) -> CommandError {
        let described = self.describe();
        let agent_code = described.agent_code;
        let exit_code_hint = described.agent_exit_code_hint;
        let mut error = CommandError::from_diagnostic(described.diagnostic, described.retryable);
        if let Some(code) = agent_code {
            error.code = code.to_owned();
        }
        error.exit_code_hint = exit_code_hint;
        error
    }
}

impl From<RuntimeError> for ErrorDiagnostic {
    fn from(error: RuntimeError) -> Self {
        error.diagnostic()
    }
}

impl From<RuntimeError> for CommandError {
    fn from(error: RuntimeError) -> Self {
        error.command_error()
    }
}
