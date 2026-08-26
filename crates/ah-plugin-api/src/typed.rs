//! The typed invocation wire, where arguments arrive already parsed against a
//! schema instead of as argv.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionContextWire {
    pub request_id: String,
    pub cwd: String,
    pub limit: Option<usize>,
    pub remaining_timeout_ms: u64,
}

impl ExecutionContextWire {
    pub fn new(
        request_id: impl Into<String>,
        cwd: impl Into<String>,
        limit: Option<usize>,
        remaining_timeout_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            cwd: cwd.into(),
            limit,
            remaining_timeout_ms,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedInvocationRequest {
    pub command: String,
    pub arguments: serde_json::Value,
    pub context: ExecutionContextWire,
    #[serde(default)]
    pub resolved_secrets: BTreeMap<String, ResolvedSecret>,
}

impl fmt::Debug for TypedInvocationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypedInvocationRequest")
            .field("command", &self.command)
            .field("arguments", &"[REDACTED]")
            .field("context", &self.context)
            .field("resolved_secrets", &self.resolved_secrets)
            .finish()
    }
}

impl TypedInvocationRequest {
    pub fn new(
        command: impl Into<String>,
        arguments: serde_json::Value,
        context: ExecutionContextWire,
    ) -> Self {
        Self {
            command: command.into(),
            arguments,
            context,
            resolved_secrets: BTreeMap::new(),
        }
    }

    pub fn with_resolved_secrets(
        mut self,
        resolved_secrets: BTreeMap<String, ResolvedSecret>,
    ) -> Self {
        self.resolved_secrets = resolved_secrets;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandNotice {
    pub code: String,
    pub message: String,
}

impl CommandNotice {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandError {
    pub domain: Option<String>,
    pub operation: Option<String>,
    pub code: String,
    pub message: String,
    pub cause: String,
    pub exit_code_hint: i32,
    pub retryable: bool,
}

impl CommandError {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        domain: Option<String>,
        operation: Option<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        cause: impl Into<String>,
        exit_code_hint: i32,
        retryable: bool,
    ) -> Self {
        Self {
            domain,
            operation,
            code: code.into(),
            message: message.into(),
            cause: cause.into(),
            exit_code_hint,
            retryable,
        }
    }

    pub fn from_diagnostic(diagnostic: ErrorDiagnostic, retryable: bool) -> Self {
        Self {
            domain: diagnostic.domain,
            operation: diagnostic.operation,
            code: diagnostic.code,
            message: diagnostic.message,
            cause: diagnostic.cause,
            exit_code_hint: diagnostic.exit_code_hint,
            retryable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedInvocationResponse {
    pub success: bool,
    pub data: Option<serde_json::Value>,
    pub text: Option<String>,
    #[serde(default)]
    pub notices: Vec<CommandNotice>,
    pub error: Option<CommandError>,
}

impl TypedInvocationResponse {
    pub fn success(data: serde_json::Value, text: Option<String>) -> Self {
        Self {
            success: true,
            data: Some(data),
            text,
            notices: Vec::new(),
            error: None,
        }
    }

    pub fn error(error: CommandError) -> Self {
        Self {
            success: false,
            data: None,
            text: None,
            notices: Vec::new(),
            error: Some(error),
        }
    }

    pub fn with_notice(mut self, notice: CommandNotice) -> Self {
        self.notices.push(notice);
        self
    }
}
