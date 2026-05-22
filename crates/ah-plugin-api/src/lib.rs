use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

use serde::{Deserialize, Serialize};

pub const AH_PLUGIN_ABI_VERSION: u32 = 1;
pub const AH_PLUGIN_API_MAJOR_VERSION: u16 = 1;
pub const AH_PLUGIN_API_MINOR_VERSION: u16 = 0;
pub const AH_PLUGIN_ENTRY_V1_SYMBOL: &[u8] = b"ah_plugin_entry_v1\0";
pub const AH_PLUGIN_METADATA_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_metadata_json_v1\0";
pub const AH_PLUGIN_MANUAL_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_manual_json_v1\0";

pub mod plugin_capabilities {
    pub const MANUAL_JSON: &str = "manual_json";
    pub const REQUIRED_TOOLS: &str = "required_tools";
    pub const ERROR_DIAGNOSTIC: &str = "error_diagnostic";
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
pub fn normalize_invocation_argv(
    argv: &[String],
    mut globals: GlobalOptionsWire,
) -> Result<InvocationNormalization, InvocationResponse> {
    let mut normalized = Vec::new();
    let mut index = 0usize;
    while index < argv.len() {
        match argv[index].as_str() {
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
    Ok(InvocationNormalization { argv: normalized, globals })
}

fn parse_limit(value: &str) -> Result<usize, InvocationResponse> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| InvocationResponse::error("INVALID_ARGUMENT", format!("invalid value for --limit: {value}")))?;
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
        self.major == AH_PLUGIN_API_MAJOR_VERSION && self.minor <= AH_PLUGIN_API_MINOR_VERSION
    }
}

impl Default for PluginApiVersion {
    fn default() -> Self {
        Self::current()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

impl Default for PluginCompatibility {
    fn default() -> Self {
        Self {
            api_version: PluginApiVersion::current(),
            capabilities: Vec::new(),
        }
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

            let created = ::std::boxed::Box::into_raw(::std::boxed::Box::new($crate::AhPluginApiV1 {
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
            let metadata = $crate::PluginMetadata {
                plugin_name: $crate::nul_terminated_bytes_to_string($plugin_name_c),
                domain: $crate::nul_terminated_bytes_to_string($domain_c),
                description: $crate::nul_terminated_bytes_to_string($description_c),
                abi_version: $crate::AH_PLUGIN_ABI_VERSION,
                required_tools: ::std::vec::Vec::new(),
                compatibility: $crate::PluginCompatibility::current()
                    .with_capability($crate::plugin_capabilities::MANUAL_JSON),
            };
            $crate::metadata_to_c_string(&metadata)
        }

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
            $crate::invoke_request_with_parser($domain, request_json, $parse_fn, $execute_fn)
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

    fn base_globals() -> GlobalOptionsWire {
        GlobalOptionsWire {
            json: false,
            quiet: false,
            limit: None,
        }
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
        let normalized = normalize_invocation_argv(&argv, base_globals())
            .expect("invocation should normalize");
        assert!(normalized.globals.json);
        assert!(normalized.globals.quiet);
        assert_eq!(normalized.globals.limit, Some(5));
        assert_eq!(
            normalized.argv,
            vec![
                "ask".to_owned(),
                "--prompt".to_owned(),
                "x".to_owned(),
            ]
        );
    }

    #[test]
    fn normalize_invocation_limit_requires_positive() {
        let argv = vec!["--limit".to_owned(), "0".to_owned()];
        let error = normalize_invocation_argv(&argv, base_globals())
            .expect_err("zero limit should fail");
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
}
