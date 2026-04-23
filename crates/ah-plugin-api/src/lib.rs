use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

use serde::{Deserialize, Serialize};

pub const AH_PLUGIN_ABI_VERSION: u32 = 1;
pub const AH_PLUGIN_ENTRY_V1_SYMBOL: &[u8] = b"ah_plugin_entry_v1\0";
pub const AH_PLUGIN_MANUAL_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_manual_json_v1\0";

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationResponse {
    pub success: bool,
    pub message: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

impl InvocationResponse {
    pub fn ok(message: Option<String>) -> Self {
        Self {
            success: true,
            message,
            error_code: None,
            error_message: None,
        }
    }

    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: None,
            error_code: Some(code.into()),
            error_message: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub plugin_name: String,
    pub domain: String,
    pub description: String,
    pub abi_version: u32,
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
pub type AhPluginManualJsonV1 = unsafe extern "C" fn() -> *mut c_char;

pub fn to_c_string_ptr(value: &str) -> *const c_char {
    let sanitized = value.replace('\0', "\\0");
    CString::new(sanitized)
        .expect("CString conversion should succeed after sanitization")
        .into_raw()
}

pub unsafe fn free_c_string_ptr(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    let _ = unsafe { CString::from_raw(value) };
}

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

pub fn null_response_ptr() -> *mut c_char {
    ptr::null_mut()
}
