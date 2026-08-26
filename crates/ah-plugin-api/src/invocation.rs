//! The untyped invocation wire: request, response, and the diagnostic a failure
//! reports.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalOptionsWire {
    pub json: bool,
    pub quiet: bool,
    pub limit: Option<usize>,
    /// The directory the request resolves relative paths against.
    ///
    /// `None` means the plugin's own process directory, which is what a host
    /// too old to send this field leaves it to. Defaulted rather than required
    /// so a host and a plugin from different releases still understand each
    /// other.
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationRequest {
    pub domain: String,
    pub argv: Vec<String>,
    pub globals: GlobalOptionsWire,
    /// Credentials the host resolved for `--credential SLOT=ID`, keyed by slot.
    /// The plugin validates that each value matches a slot it actually accepts.
    #[serde(default)]
    pub resolved_secrets: BTreeMap<String, ResolvedSecret>,
}

impl InvocationRequest {
    pub fn new(domain: impl Into<String>, argv: Vec<String>, globals: GlobalOptionsWire) -> Self {
        Self {
            domain: domain.into(),
            argv,
            globals,
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
pub struct InvocationNormalization {
    pub argv: Vec<String>,
    pub globals: GlobalOptionsWire,
}

/// Normalizes invocation arguments before plugin parsing, extracting supported
/// global flags from `argv` into `globals`.
pub fn normalize_invocation_argv(
    argv: &[String],
    mut globals: GlobalOptionsWire,
) -> Result<InvocationNormalization, InvocationResponse> {
    let mut normalized = Vec::new();
    let mut index = 0usize;
    while index < argv.len() {
        match argv[index].as_str() {
            "--" => {
                normalized.extend_from_slice(&argv[index..]);
                break;
            }
            "--json" => {
                globals.json = true;
                index += 1;
            }
            "--quiet" => {
                globals.quiet = true;
                index += 1;
            }
            "--limit" => {
                let value = argv.get(index + 1).ok_or_else(|| {
                    InvocationResponse::error(
                        "INVALID_ARGUMENT",
                        "missing value for trailing --limit",
                    )
                })?;
                let parsed = parse_limit(value)?;
                globals.limit = Some(parsed);
                index += 2;
            }
            _ => {
                if let Some(value) = argv[index].strip_prefix("--limit=") {
                    globals.limit = Some(parse_limit(value)?);
                    index += 1;
                } else {
                    normalized.push(argv[index].to_owned());
                    index += 1;
                }
            }
        }
    }
    Ok(InvocationNormalization {
        argv: normalized,
        globals,
    })
}

pub(super) fn parse_limit(value: &str) -> Result<usize, InvocationResponse> {
    let parsed = value.parse::<usize>().map_err(|_| {
        InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("invalid value for --limit: {value}"),
        )
    })?;
    if parsed == 0 {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--limit must be >= 1",
        ));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    // Boxed: an inline diagnostic more than doubles the size of every
    // `InvocationResponse`, which every plugin entry point returns by value.
    pub diagnostic: Option<Box<ErrorDiagnostic>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorDiagnostic {
    pub domain: Option<String>,
    pub operation: Option<String>,
    pub code: String,
    pub message: String,
    pub cause: String,
    pub exit_code_hint: i32,
}

impl ErrorDiagnostic {
    pub fn new(
        domain: Option<String>,
        operation: Option<String>,
        code: impl Into<String>,
        message: impl Into<String>,
        cause: impl Into<String>,
        exit_code_hint: i32,
    ) -> Self {
        Self {
            domain,
            operation,
            code: code.into(),
            message: message.into(),
            cause: cause.into(),
            exit_code_hint,
        }
    }

    pub fn with_domain(mut self, domain: impl Into<String>) -> Self {
        if self.domain.is_none() {
            self.domain = Some(domain.into());
        }
        self
    }

    pub fn with_operation(mut self, operation: impl Into<String>) -> Self {
        if self.operation.is_none() {
            self.operation = Some(operation.into());
        }
        self
    }
}

impl std::fmt::Display for ErrorDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl InvocationResponse {
    pub fn ok(message: Option<String>) -> Self {
        Self {
            success: true,
            message,
            error_code: None,
            error_message: None,
            diagnostic: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let message = message.into();
        Self {
            success: false,
            message: None,
            error_code: Some(code.clone()),
            error_message: Some(message.clone()),
            diagnostic: Some(Box::new(ErrorDiagnostic::new(
                None,
                None,
                code,
                message.clone(),
                message,
                1,
            ))),
        }
    }

    pub fn error_diagnostic(diagnostic: ErrorDiagnostic) -> Self {
        Self {
            success: false,
            message: None,
            error_code: Some(diagnostic.code.clone()),
            error_message: Some(diagnostic.message.clone()),
            diagnostic: Some(Box::new(diagnostic)),
        }
    }

    pub fn with_error_domain(mut self, domain: impl Into<String>) -> Self {
        if let Some(diagnostic) = self.diagnostic.take() {
            self.diagnostic = Some(Box::new(diagnostic.with_domain(domain)));
        }
        self
    }
}
