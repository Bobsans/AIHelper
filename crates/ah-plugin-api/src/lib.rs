use std::{
    ffi::{CStr, CString, c_char},
    fmt::Display,
    io::{self, IsTerminal},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

use serde::{Deserialize, Serialize};

pub const AH_PLUGIN_ABI_VERSION: u32 = 1;
pub const AH_PLUGIN_API_MAJOR_VERSION: u16 = 1;
pub const AH_PLUGIN_API_MINOR_VERSION: u16 = 1;
pub const AH_PLUGIN_ENTRY_V1_SYMBOL: &[u8] = b"ah_plugin_entry_v1\0";
pub const AH_PLUGIN_METADATA_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_metadata_json_v1\0";
pub const AH_PLUGIN_MANUAL_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_manual_json_v1\0";
pub const AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_command_catalog_json_v1\0";
pub const AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_invoke_command_json_v1\0";
pub const AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL: &[u8] = b"ah_plugin_cancel_command_v1\0";

pub mod plugin_capabilities {
    pub const MANUAL_JSON: &str = "manual_json";
    pub const REQUIRED_TOOLS: &str = "required_tools";
    pub const ERROR_DIAGNOSTIC: &str = "error_diagnostic";
    pub const TYPED_COMMANDS_V1: &str = "typed_commands_v1";
}

const ANSI_RESET: &str = "\u{1b}[0m";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum TextStyle {
    Heading,
    Key,
    Success,
    Warning,
    Error,
    Muted,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct TextFormatter {
    color: bool,
}

impl TextFormatter {
    pub fn stdout() -> Self {
        Self::automatic(io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::automatic(io::stderr().is_terminal())
    }

    pub const fn with_color(color: bool) -> Self {
        Self { color }
    }

    pub fn paint(self, style: TextStyle, value: impl Display) -> String {
        if !self.color {
            return value.to_string();
        }

        format!("{}{value}{ANSI_RESET}", ansi_prefix(style))
    }

    fn automatic(is_terminal: bool) -> Self {
        Self::with_color(color_enabled(
            is_terminal,
            std::env::var_os("NO_COLOR").is_some(),
        ))
    }
}

const fn color_enabled(is_terminal: bool, no_color: bool) -> bool {
    is_terminal && !no_color
}

const fn ansi_prefix(style: TextStyle) -> &'static str {
    match style {
        TextStyle::Heading => "\u{1b}[1;36m",
        TextStyle::Key => "\u{1b}[36m",
        TextStyle::Success => "\u{1b}[32m",
        TextStyle::Warning => "\u{1b}[33m",
        TextStyle::Error => "\u{1b}[1;31m",
        TextStyle::Muted => "\u{1b}[2m",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GlobalOptionsWire {
    pub json: bool,
    pub quiet: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationRequest {
    pub domain: String,
    pub argv: Vec<String>,
    pub globals: GlobalOptionsWire,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvocationNormalization {
    pub argv: Vec<String>,
    pub globals: GlobalOptionsWire,
}

/// Normalizes invocation arguments before plugin parsing, extracting supported
/// global flags from `argv` into `globals`.
#[allow(clippy::result_large_err)]
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

#[allow(clippy::result_large_err)]
fn parse_limit(value: &str) -> Result<usize, InvocationResponse> {
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
    pub diagnostic: Option<ErrorDiagnostic>,
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
            diagnostic: Some(ErrorDiagnostic::new(
                None,
                None,
                code,
                message.clone(),
                message,
                1,
            )),
        }
    }

    pub fn error_diagnostic(diagnostic: ErrorDiagnostic) -> Self {
        Self {
            success: false,
            message: None,
            error_code: Some(diagnostic.code.clone()),
            error_message: Some(diagnostic.message.clone()),
            diagnostic: Some(diagnostic),
        }
    }

    pub fn with_error_domain(mut self, domain: impl Into<String>) -> Self {
        if let Some(diagnostic) = self.diagnostic.take() {
            self.diagnostic = Some(diagnostic.with_domain(domain));
        }
        self
    }

    pub fn with_error_operation(mut self, operation: impl Into<String>) -> Self {
        if let Some(diagnostic) = self.diagnostic.take() {
            self.diagnostic = Some(diagnostic.with_operation(operation));
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandEffect {
    FilesystemRead,
    FilesystemWrite,
    FilesystemDelete,
    ProcessSpawn,
    NetworkRead,
    NetworkWrite,
    ConfigurationRead,
    ConfigurationWrite,
    ExternalRead,
    ExternalWrite,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandEffects {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
    pub effects: Vec<CommandEffect>,
    pub risk: RiskLevel,
    pub impact: String,
    pub reversibility: Reversibility,
}

impl CommandEffects {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        read_only: bool,
        destructive: bool,
        idempotent: bool,
        open_world: bool,
        effects: Vec<CommandEffect>,
        risk: RiskLevel,
        impact: impl Into<String>,
        reversibility: Reversibility,
    ) -> Self {
        Self {
            read_only,
            destructive,
            idempotent,
            open_world,
            effects,
            risk,
            impact: impact.into(),
            reversibility,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandExample {
    pub description: String,
    pub arguments: serde_json::Value,
}

impl CommandExample {
    pub fn new(description: impl Into<String>, arguments: serde_json::Value) -> Self {
        Self {
            description: description.into(),
            arguments,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandDescriptor {
    pub id: String,
    pub title: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub effects: CommandEffects,
    #[serde(default)]
    pub examples: Vec<CommandExample>,
}

impl CommandDescriptor {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        description: impl Into<String>,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
        effects: CommandEffects,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: description.into(),
            input_schema,
            output_schema,
            effects,
            examples: Vec::new(),
        }
    }

    pub fn with_example(mut self, example: CommandExample) -> Self {
        self.examples.push(example);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandCatalog {
    pub plugin_name: String,
    pub domain: String,
    pub commands: Vec<CommandDescriptor>,
}

impl CommandCatalog {
    pub fn new(
        plugin_name: impl Into<String>,
        domain: impl Into<String>,
        commands: Vec<CommandDescriptor>,
    ) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            domain: domain.into(),
            commands,
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypedInvocationRequest {
    pub command: String,
    pub arguments: serde_json::Value,
    pub context: ExecutionContextWire,
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
        }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl PluginApiVersion {
    pub fn current() -> Self {
        Self {
            major: AH_PLUGIN_API_MAJOR_VERSION,
            minor: AH_PLUGIN_API_MINOR_VERSION,
        }
    }

    pub fn is_compatible_with_host(&self) -> bool {
        let host = Self::current();
        self.major == host.major && self.minor <= host.minor
    }
}

impl Default for PluginApiVersion {
    fn default() -> Self {
        Self::current()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginCompatibility {
    #[serde(default)]
    pub api_version: PluginApiVersion,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl PluginCompatibility {
    pub fn current() -> Self {
        Self::default()
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities
            .iter()
            .any(|candidate| candidate == capability)
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        let capability = capability.into();
        if !self.supports(&capability) {
            self.capabilities.push(capability);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub plugin_name: String,
    pub domain: String,
    pub description: String,
    pub abi_version: u32,
    #[serde(default)]
    pub required_tools: Vec<RequiredTool>,
    #[serde(default)]
    pub compatibility: PluginCompatibility,
}

impl PluginMetadata {
    pub fn supports_capability(&self, capability: &str) -> bool {
        self.compatibility.supports(capability)
    }

    pub fn is_api_compatible_with_host(&self) -> bool {
        self.compatibility.api_version.is_compatible_with_host()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequiredTool {
    pub name: String,
    pub check_args: Vec<String>,
    pub reason: String,
}

impl RequiredTool {
    pub fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            check_args: vec!["--version".to_owned()],
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualExample {
    pub description: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualCommand {
    pub name: String,
    pub summary: String,
    pub usage: String,
    pub examples: Vec<ManualExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManual {
    pub plugin_name: String,
    pub domain: String,
    pub description: String,
    pub commands: Vec<ManualCommand>,
    pub notes: Vec<String>,
}

#[repr(C)]
pub struct AhPluginApiV1 {
    pub abi_version: u32,
    pub plugin_name: *const c_char,
    pub domain: *const c_char,
    pub description: *const c_char,
    pub invoke_json: unsafe extern "C" fn(request_json: *const c_char) -> *mut c_char,
    pub free_c_string: unsafe extern "C" fn(value: *mut c_char),
}

pub type AhPluginEntryV1 = unsafe extern "C" fn() -> *const AhPluginApiV1;
pub type AhPluginMetadataJsonV1 = unsafe extern "C" fn() -> *mut c_char;
pub type AhPluginManualJsonV1 = unsafe extern "C" fn() -> *mut c_char;
pub type AhPluginCommandCatalogJsonV1 = unsafe extern "C" fn() -> *mut c_char;
pub type AhPluginInvokeCommandJsonV1 =
    unsafe extern "C" fn(request_json: *const c_char) -> *mut c_char;
pub type AhPluginCancelCommandV1 = unsafe extern "C" fn(request_id: *const c_char) -> i32;

pub fn to_c_string_ptr(value: &str) -> *const c_char {
    let sanitized = value.replace('\0', "\\0");
    CString::new(sanitized)
        .expect("CString conversion should succeed after sanitization")
        .into_raw()
}

/// Frees a C string pointer previously returned by this API.
///
/// # Safety
///
/// `value` must be null or a pointer produced by `CString::into_raw` from this
/// crate. Passing any other pointer, or freeing the same pointer more than
/// once, is undefined behavior.
pub unsafe fn free_c_string_ptr(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    let _ = unsafe { CString::from_raw(value) };
}

/// Converts a raw invocation request into a typed response using the plugin-local
/// argument parser and command executor.
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub fn invoke_request_with_parser<TArgs, TParse, TExecute>(
    expected_domain: &str,
    request_json: *const c_char,
    parse_args: TParse,
    execute: TExecute,
) -> InvocationResponse
where
    TParse: Fn(&[String]) -> Result<TArgs, InvocationResponse>,
    TExecute: Fn(TArgs, &GlobalOptionsWire) -> InvocationResponse,
{
    let request_json = match unsafe { c_ptr_to_string(request_json) } {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request pointer: {error}"),
            );
        }
    };

    let request = match serde_json::from_str::<InvocationRequest>(&request_json) {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request JSON: {error}"),
            );
        }
    };

    if request.domain != expected_domain {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!(
                "plugin domain mismatch: expected '{expected_domain}', got '{}'",
                request.domain
            ),
        );
    }

    let normalized = match normalize_invocation_argv(&request.argv, request.globals) {
        Ok(value) => value,
        Err(error) => return error.with_error_domain(expected_domain),
    };

    let parsed = match parse_args(&normalized.argv) {
        Ok(value) => value,
        Err(response) => return response.with_error_domain(expected_domain),
    };

    execute(parsed, &normalized.globals).with_error_domain(expected_domain)
}

/// Runs a plugin parser and executor without allowing an unwind to cross the C ABI boundary.
#[allow(clippy::result_large_err)]
pub fn invoke_request_with_parser_catch_unwind<TArgs, TParse, TExecute>(
    expected_domain: &str,
    request_json: *const c_char,
    parse_args: TParse,
    execute: TExecute,
) -> InvocationResponse
where
    TParse: Fn(&[String]) -> Result<TArgs, InvocationResponse>,
    TExecute: Fn(TArgs, &GlobalOptionsWire) -> InvocationResponse,
{
    match catch_unwind(AssertUnwindSafe(|| {
        invoke_request_with_parser(expected_domain, request_json, parse_args, execute)
    })) {
        Ok(response) => response,
        Err(_) => InvocationResponse::error(
            "PLUGIN_PANIC",
            format!("plugin '{expected_domain}' panicked while handling invocation"),
        )
        .with_error_domain(expected_domain),
    }
}

/// Generates the standardized ABI surface for an `AhPluginApiV1` dynamic plugin.
#[macro_export]
macro_rules! define_plugin_entrypoint_v1 {
    (
        plugin_name_c: $plugin_name_c:expr,
        domain_c: $domain_c:expr,
        description_c: $description_c:expr,
        domain: $domain:expr,
        parse_fn: $parse_fn:path,
        execute_fn: $execute_fn:path,
        manual_fn: $manual_fn:path,
        $(
            typed_catalog_fn: $typed_catalog_fn:path,
            typed_execute_fn: $typed_execute_fn:path,
            typed_cancel_fn: $typed_cancel_fn:path,
        )?
    ) => {
        static PLUGIN_API_PTR: ::std::sync::atomic::AtomicPtr<$crate::AhPluginApiV1> =
            ::std::sync::atomic::AtomicPtr::new(::std::ptr::null_mut());

        /// Returns the plugin ABI entry point.
        ///
        /// # Safety
        ///
        /// The returned pointer is process-static and must not be freed or mutated
        /// by the caller.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn ah_plugin_entry_v1() -> *const $crate::AhPluginApiV1 {
            let existing = PLUGIN_API_PTR.load(::std::sync::atomic::Ordering::Acquire);
            if !existing.is_null() {
                return existing.cast_const();
            }

            let created =
                ::std::boxed::Box::into_raw(::std::boxed::Box::new($crate::AhPluginApiV1 {
                    abi_version: $crate::AH_PLUGIN_ABI_VERSION,
                    plugin_name: $plugin_name_c.as_ptr().cast(),
                    domain: $domain_c.as_ptr().cast(),
                    description: $description_c.as_ptr().cast(),
                    invoke_json: ah_plugin_invoke_json,
                    free_c_string: ah_plugin_free_c_string,
                }));

            match PLUGIN_API_PTR.compare_exchange(
                ::std::ptr::null_mut(),
                created,
                ::std::sync::atomic::Ordering::AcqRel,
                ::std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => created.cast_const(),
                Err(existing) => {
                    unsafe { drop(::std::boxed::Box::from_raw(created)) };
                    existing.cast_const()
                }
            }
        }

        /// Returns the plugin manual JSON as an owned C string.
        ///
        /// # Safety
        ///
        /// The returned pointer must be freed through `free_c_string`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn ah_plugin_manual_json_v1() -> *mut ::std::os::raw::c_char {
            $crate::manual_to_c_string(&$manual_fn())
        }

        /// Returns the plugin metadata JSON as an owned C string.
        ///
        /// # Safety
        ///
        /// The returned pointer must be freed through `free_c_string`.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn ah_plugin_metadata_json_v1() -> *mut ::std::os::raw::c_char {
            let mut compatibility = $crate::PluginCompatibility::current()
                .with_capability($crate::plugin_capabilities::MANUAL_JSON);
            $(
                let _ = ::std::stringify!($typed_catalog_fn);
                compatibility = compatibility
                    .with_capability($crate::plugin_capabilities::TYPED_COMMANDS_V1);
            )?
            let metadata = $crate::PluginMetadata {
                plugin_name: $crate::nul_terminated_bytes_to_string($plugin_name_c),
                domain: $crate::nul_terminated_bytes_to_string($domain_c),
                description: $crate::nul_terminated_bytes_to_string($description_c),
                abi_version: $crate::AH_PLUGIN_ABI_VERSION,
                required_tools: ::std::vec::Vec::new(),
                compatibility,
            };
            $crate::metadata_to_c_string(&metadata)
        }

        $(
            /// Returns the typed plugin command catalog as an owned C string.
            ///
            /// # Safety
            ///
            /// The returned pointer must be freed through `free_c_string`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ah_plugin_command_catalog_json_v1(
            ) -> *mut ::std::os::raw::c_char {
                match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                    $typed_catalog_fn()
                })) {
                    Ok(catalog) => $crate::command_catalog_to_c_string(&catalog),
                    Err(_) => ::std::ptr::null_mut(),
                }
            }

            /// Invokes one typed plugin command.
            ///
            /// # Safety
            ///
            /// `request_json` must be a valid C string owned by the caller.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ah_plugin_invoke_command_json_v1(
                request_json: *const ::std::os::raw::c_char,
            ) -> *mut ::std::os::raw::c_char {
                let response = match ::std::panic::catch_unwind(
                    ::std::panic::AssertUnwindSafe(|| {
                        let request =
                            unsafe { $crate::typed_request_from_c_ptr(request_json) }?;
                        Ok::<$crate::TypedInvocationResponse, ::std::string::String>(
                            $typed_execute_fn(&request),
                        )
                    }),
                ) {
                    Ok(Ok(response)) => response,
                    Ok(Err(error)) => $crate::TypedInvocationResponse::error(
                        $crate::CommandError::new(
                            Some($domain.to_owned()),
                            None,
                            "INVALID_TYPED_REQUEST",
                            "failed to decode typed plugin request",
                            error,
                            2,
                            false,
                        ),
                    ),
                    Err(_) => $crate::TypedInvocationResponse::error($crate::CommandError::new(
                        Some($domain.to_owned()),
                        None,
                        "PLUGIN_PANIC",
                        "plugin panicked while handling typed invocation",
                        "panic was caught at the dynamic plugin ABI boundary",
                        1,
                        false,
                    )),
                };
                $crate::typed_response_to_c_string(&response)
            }

            /// Requests cancellation of one typed plugin invocation.
            ///
            /// # Safety
            ///
            /// `request_id` must be a valid C string owned by the caller.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn ah_plugin_cancel_command_v1(
                request_id: *const ::std::os::raw::c_char,
            ) -> i32 {
                match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                    let request_id = unsafe { $crate::c_ptr_to_string(request_id) }.ok()?;
                    Some($typed_cancel_fn(&request_id))
                })) {
                    Ok(Some(true)) => 1,
                    _ => 0,
                }
            }
        )?

        unsafe extern "C" fn ah_plugin_invoke_json(
            request_json: *const ::std::os::raw::c_char,
        ) -> *mut ::std::os::raw::c_char {
            let response = invoke_from_raw(request_json);
            $crate::response_to_c_string(&response)
        }

        unsafe extern "C" fn ah_plugin_free_c_string(value: *mut ::std::os::raw::c_char) {
            unsafe { $crate::free_c_string_ptr(value) };
        }

        fn invoke_from_raw(
            request_json: *const ::std::os::raw::c_char,
        ) -> $crate::InvocationResponse {
            $crate::invoke_request_with_parser_catch_unwind(
                $domain,
                request_json,
                $parse_fn,
                $execute_fn,
            )
        }
    };
}

/// Converts a non-null C string pointer to a Rust `String`.
///
/// # Safety
///
/// `ptr_value` must point to a valid, nul-terminated C string for the duration
/// of this call.
pub unsafe fn c_ptr_to_string(ptr_value: *const c_char) -> Result<String, String> {
    if ptr_value.is_null() {
        return Err("null c string pointer".to_owned());
    }
    let c_str = unsafe { CStr::from_ptr(ptr_value) };
    c_str
        .to_str()
        .map(str::to_owned)
        .map_err(|error| format!("invalid utf8 in c string: {error}"))
}

/// Decodes a typed invocation request from a borrowed C string.
///
/// # Safety
///
/// `request_json` must point to a valid, nul-terminated C string for the
/// duration of this call.
pub unsafe fn typed_request_from_c_ptr(
    request_json: *const c_char,
) -> Result<TypedInvocationRequest, String> {
    let raw = unsafe { c_ptr_to_string(request_json) }?;
    serde_json::from_str(&raw).map_err(|error| error.to_string())
}

pub fn response_to_c_string(response: &InvocationResponse) -> *mut c_char {
    match serde_json::to_string(response) {
        Ok(raw) => CString::new(raw)
            .expect("JSON should not contain interior null bytes")
            .into_raw(),
        Err(error) => {
            let fallback = format!(
                "{{\"success\":false,\"error_code\":\"JSON_SERIALIZATION_FAILED\",\"error_message\":\"{}\"}}",
                error.to_string().replace('"', "'")
            );
            CString::new(fallback)
                .expect("fallback JSON must be valid cstring")
                .into_raw()
        }
    }
}

pub fn manual_to_c_string(manual: &PluginManual) -> *mut c_char {
    match serde_json::to_string(manual) {
        Ok(raw) => CString::new(raw)
            .expect("JSON should not contain interior null bytes")
            .into_raw(),
        Err(_) => null_response_ptr(),
    }
}

pub fn metadata_to_c_string(metadata: &PluginMetadata) -> *mut c_char {
    match serde_json::to_string(metadata) {
        Ok(raw) => CString::new(raw)
            .expect("JSON should not contain interior null bytes")
            .into_raw(),
        Err(_) => null_response_ptr(),
    }
}

pub fn command_catalog_to_c_string(catalog: &CommandCatalog) -> *mut c_char {
    match serde_json::to_string(catalog) {
        Ok(raw) => CString::new(raw)
            .expect("JSON should not contain interior null bytes")
            .into_raw(),
        Err(_) => null_response_ptr(),
    }
}

pub fn typed_response_to_c_string(response: &TypedInvocationResponse) -> *mut c_char {
    match serde_json::to_string(response) {
        Ok(raw) => CString::new(raw)
            .expect("JSON should not contain interior null bytes")
            .into_raw(),
        Err(error) => {
            let fallback = TypedInvocationResponse::error(CommandError::new(
                None,
                None,
                "JSON_SERIALIZATION_FAILED",
                "failed to serialize typed plugin response",
                error.to_string(),
                1,
                false,
            ));
            let raw = serde_json::to_string(&fallback)
                .expect("typed serialization fallback should always serialize");
            CString::new(raw)
                .expect("fallback JSON must be a valid cstring")
                .into_raw()
        }
    }
}

pub fn nul_terminated_bytes_to_string(value: &[u8]) -> String {
    CStr::from_bytes_with_nul(value)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn null_response_ptr() -> *mut c_char {
    ptr::null_mut()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_applies_semantic_ansi_style_when_enabled() {
        let formatter = TextFormatter::with_color(true);

        assert_eq!(
            formatter.paint(TextStyle::Success, "enabled"),
            "\u{1b}[32menabled\u{1b}[0m"
        );
        assert_eq!(
            formatter.paint(TextStyle::Error, "ERROR"),
            "\u{1b}[1;31mERROR\u{1b}[0m"
        );
    }

    #[test]
    fn formatter_preserves_plain_text_when_disabled() {
        let formatter = TextFormatter::with_color(false);

        assert_eq!(formatter.paint(TextStyle::Heading, "DOMAIN"), "DOMAIN");
    }

    #[test]
    fn automatic_color_requires_terminal_and_no_no_color_request() {
        assert!(color_enabled(true, false));
        assert!(!color_enabled(false, false));
        assert!(!color_enabled(true, true));
    }

    fn base_globals() -> GlobalOptionsWire {
        GlobalOptionsWire {
            json: false,
            quiet: false,
            limit: None,
        }
    }

    fn sample_descriptor() -> CommandDescriptor {
        CommandDescriptor::new(
            "test.inspect",
            "Inspect test data",
            "Inspect test data without modifying it.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"],
                "additionalProperties": false
            }),
            CommandEffects::new(
                true,
                false,
                true,
                false,
                vec![CommandEffect::FilesystemRead],
                RiskLevel::Low,
                "Reads test data.",
                Reversibility::Yes,
            ),
        )
        .with_example(CommandExample::new(
            "Inspect one value",
            serde_json::json!({ "value": "demo" }),
        ))
    }

    #[test]
    fn current_plugin_api_version_is_1_1() {
        assert_eq!(
            PluginApiVersion::current(),
            PluginApiVersion { major: 1, minor: 1 }
        );
    }

    #[test]
    fn typed_command_contract_round_trips() {
        let catalog = CommandCatalog::new("test-plugin", "test", vec![sample_descriptor()]);
        let raw = serde_json::to_string(&catalog).expect("catalog should serialize");
        let decoded =
            serde_json::from_str::<CommandCatalog>(&raw).expect("catalog should deserialize");

        assert_eq!(decoded, catalog);
        assert_eq!(decoded.commands[0].effects.risk, RiskLevel::Low);
        assert_eq!(
            serde_json::to_value(CommandEffect::FilesystemRead).expect("effect should serialize"),
            serde_json::json!("filesystem_read")
        );
    }

    #[test]
    fn typed_invocation_success_and_error_are_exclusive() {
        let success = TypedInvocationResponse::success(
            serde_json::json!({ "value": "ok" }),
            Some("ok".to_owned()),
        )
        .with_notice(CommandNotice::new("TRUNCATED", "Output was truncated."));
        assert!(success.success);
        assert!(success.data.is_some());
        assert!(success.error.is_none());
        assert_eq!(success.notices.len(), 1);

        let error = TypedInvocationResponse::error(CommandError::new(
            Some("test".to_owned()),
            Some("test.inspect".to_owned()),
            "INVALID_ARGUMENT",
            "value is required",
            "missing field",
            2,
            false,
        ));
        assert!(!error.success);
        assert!(error.data.is_none());
        assert_eq!(
            error.error.as_ref().map(|value| value.code.as_str()),
            Some("INVALID_ARGUMENT")
        );
    }

    #[test]
    fn typed_response_c_string_contains_valid_json() {
        let response = TypedInvocationResponse::success(serde_json::json!({ "value": "ok" }), None);
        let ptr = typed_response_to_c_string(&response);
        assert!(!ptr.is_null());

        let raw = unsafe { c_ptr_to_string(ptr.cast_const()) }
            .expect("typed response should be valid utf8");
        unsafe { free_c_string_ptr(ptr) };
        let decoded = serde_json::from_str::<TypedInvocationResponse>(&raw)
            .expect("typed response should be valid JSON");
        assert_eq!(decoded, response);
    }

    #[test]
    fn normalize_invocation_handles_json_quiet_and_limit() {
        let argv = vec![
            "--json".to_owned(),
            "ask".to_owned(),
            "--quiet".to_owned(),
            "--limit".to_owned(),
            "7".to_owned(),
            "--prompt".to_owned(),
            "x".to_owned(),
            "--limit=5".to_owned(),
        ];
        let normalized =
            normalize_invocation_argv(&argv, base_globals()).expect("invocation should normalize");
        assert!(normalized.globals.json);
        assert!(normalized.globals.quiet);
        assert_eq!(normalized.globals.limit, Some(5));
        assert_eq!(
            normalized.argv,
            vec!["ask".to_owned(), "--prompt".to_owned(), "x".to_owned(),]
        );
    }

    #[test]
    fn normalize_invocation_limit_requires_positive() {
        let argv = vec!["--limit".to_owned(), "0".to_owned()];
        let error =
            normalize_invocation_argv(&argv, base_globals()).expect_err("zero limit should fail");
        assert_eq!(error.error_code.as_deref(), Some("INVALID_ARGUMENT"));
    }

    #[test]
    fn normalize_invocation_limit_requires_value() {
        let argv = vec!["ask".to_owned(), "--limit".to_owned()];
        let error = normalize_invocation_argv(&argv, base_globals())
            .expect_err("missing limit value should fail");
        assert_eq!(error.error_code.as_deref(), Some("INVALID_ARGUMENT"));
    }

    #[test]
    fn normalize_invocation_limit_requires_numeric_value() {
        let argv = vec!["ask".to_owned(), "--limit=soon".to_owned()];
        let error = normalize_invocation_argv(&argv, base_globals())
            .expect_err("non-numeric limit should fail");
        assert_eq!(error.error_code.as_deref(), Some("INVALID_ARGUMENT"));
    }

    #[test]
    fn normalize_invocation_ignores_unrelated_args() {
        let argv = vec!["ask".to_owned(), "--prompt".to_owned(), "x".to_owned()];
        let normalized = normalize_invocation_argv(&argv, base_globals()).expect("no-op normalize");
        assert_eq!(normalized.argv, argv);
        assert_eq!(normalized.globals, base_globals());
    }

    #[test]
    fn normalize_invocation_preserves_opaque_suffix() {
        let argv = vec![
            "check".to_owned(),
            "--".to_owned(),
            "child".to_owned(),
            "--json".to_owned(),
            "--limit".to_owned(),
            "invalid-for-host".to_owned(),
            "--cwd".to_owned(),
            "nested".to_owned(),
        ];
        let normalized = normalize_invocation_argv(&argv, base_globals())
            .expect("opaque suffix should not be normalized");

        assert_eq!(normalized.argv, argv);
        assert_eq!(normalized.globals, base_globals());
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn plugin_parser_panic_becomes_structured_error() {
        let request = InvocationRequest {
            domain: "test".to_owned(),
            argv: vec!["run".to_owned()],
            globals: base_globals(),
        };
        let raw = CString::new(serde_json::to_string(&request).expect("request should serialize"))
            .expect("request should be a cstring");
        let response = invoke_request_with_parser_catch_unwind(
            "test",
            raw.as_ptr(),
            |_| -> Result<(), InvocationResponse> { panic!("parser secret") },
            |_, _| InvocationResponse::ok(None),
        );

        assert_eq!(response.error_code.as_deref(), Some("PLUGIN_PANIC"));
        assert_eq!(
            response.error_message.as_deref(),
            Some("plugin 'test' panicked while handling invocation")
        );
        assert_eq!(
            response.diagnostic.and_then(|value| value.domain),
            Some("test".to_owned())
        );
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn plugin_executor_panic_does_not_poison_later_invocation() {
        let request = InvocationRequest {
            domain: "test".to_owned(),
            argv: vec!["run".to_owned()],
            globals: base_globals(),
        };
        let raw = CString::new(serde_json::to_string(&request).expect("request should serialize"))
            .expect("request should be a cstring");
        let panic_response = invoke_request_with_parser_catch_unwind(
            "test",
            raw.as_ptr(),
            |_| Ok(()),
            |_, _| -> InvocationResponse { panic!("executor secret") },
        );
        assert_eq!(panic_response.error_code.as_deref(), Some("PLUGIN_PANIC"));

        let success_response = invoke_request_with_parser_catch_unwind(
            "test",
            raw.as_ptr(),
            |_| Ok(()),
            |_, _| InvocationResponse::ok(Some("ok".to_owned())),
        );
        assert!(success_response.success);
        assert_eq!(success_response.message.as_deref(), Some("ok"));
    }
}
