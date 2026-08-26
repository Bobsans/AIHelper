//! The C ABI itself: the symbol names a plugin must export, the function
//! signatures behind them, and the conversions across the boundary.
//!
//! Nothing here may change shape without a version negotiation - a plugin built
//! against an older header has to keep working.

use super::*;

pub const AH_PLUGIN_ABI_VERSION: u32 = 1;

pub const AH_PLUGIN_API_MAJOR_VERSION: u16 = 1;

pub const AH_PLUGIN_API_MINOR_VERSION: u16 = 1;

pub const AH_PLUGIN_ENTRY_V1_SYMBOL: &[u8] = b"ah_plugin_entry_v1\0";

pub const AH_PLUGIN_METADATA_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_metadata_json_v1\0";

pub const AH_PLUGIN_MANUAL_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_manual_json_v1\0";

pub const AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_command_catalog_json_v1\0";

pub const AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL: &[u8] = b"ah_plugin_invoke_command_json_v1\0";

pub const AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL: &[u8] = b"ah_plugin_cancel_command_v1\0";

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
