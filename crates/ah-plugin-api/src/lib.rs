//! The plugin ABI, and the Rust-side conveniences over it.
//!
//! `abi` is the contract: a plugin built against an older header must keep
//! working, so nothing there changes shape without version negotiation.
//! Everything else is the payload that crosses it.
//!
//! | Module       | Owns                                                     |
//! |--------------|----------------------------------------------------------|
//! | `abi`        | exported symbols, signatures, and the C conversions      |
//! | `invocation` | the untyped request/response wire                        |
//! | `typed`      | the typed wire, arguments parsed against a schema        |
//! | `catalog`    | what a plugin says it can do, and what that costs        |
//! | `metadata`   | who the plugin is, and the manual it ships               |
//! | `text`       | styled terminal output without a colour crate            |
//! | `sdk`        | Rust-only sugar; none of it crosses the boundary         |
//!
//! The 1259 production lines these came from were one file, which made the ABI
//! surface - the part that cannot change - indistinguishable from the parts
//! that can.

use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, OsStr, c_char},
    fmt::{self, Display},
    io::{self, IsTerminal},
    panic::{AssertUnwindSafe, catch_unwind},
    process::Command,
    ptr,
};

use serde::{Deserialize, Serialize};

mod abi;
mod catalog;
mod invocation;
mod metadata;
mod sdk;
mod text;
mod typed;
pub use abi::{
    AH_PLUGIN_ABI_VERSION, AH_PLUGIN_API_MAJOR_VERSION, AH_PLUGIN_API_MINOR_VERSION,
    AH_PLUGIN_CANCEL_COMMAND_V1_SYMBOL, AH_PLUGIN_COMMAND_CATALOG_JSON_V1_SYMBOL,
    AH_PLUGIN_ENTRY_V1_SYMBOL, AH_PLUGIN_INVOKE_COMMAND_JSON_V1_SYMBOL,
    AH_PLUGIN_MANUAL_JSON_V1_SYMBOL, AH_PLUGIN_METADATA_JSON_V1_SYMBOL, AhPluginApiV1,
    AhPluginCancelCommandV1, AhPluginCommandCatalogJsonV1, AhPluginEntryV1,
    AhPluginInvokeCommandJsonV1, AhPluginManualJsonV1, AhPluginMetadataJsonV1, c_ptr_to_string,
    command_catalog_to_c_string, free_c_string_ptr, manual_to_c_string, metadata_to_c_string,
    nul_terminated_bytes_to_string, null_response_ptr, response_to_c_string,
    typed_request_from_c_ptr, typed_response_to_c_string,
};
pub use catalog::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandExample,
    ResolvedSecret, Reversibility, RiskLevel, SecretSlot,
};
pub use invocation::{
    ErrorDiagnostic, GlobalOptionsWire, InvocationNormalization, InvocationRequest,
    InvocationResponse, normalize_invocation_argv,
};
pub use metadata::{
    ManualCommand, ManualExample, PluginApiVersion, PluginCompatibility, PluginManual,
    PluginMetadata, RequiredTool,
};
pub use sdk::{
    BindResolvedSecrets, invoke_request_with_parser, invoke_request_with_parser_catch_unwind,
    noninteractive_command, plugin_capabilities,
};
pub use text::{TextFormatter, TextStyle};
pub use typed::{
    CommandError, CommandNotice, ExecutionContextWire, TypedInvocationRequest,
    TypedInvocationResponse,
};

pub const AH_VAULT_MASTER_KEY_ENV: &str = "AH_VAULT_MASTER_KEY";

pub mod cancellation;

pub mod schema;

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

impl fmt::Debug for ResolvedSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedSecret")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("values", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_typed_debug_never_exposes_values_or_arguments() {
        let sentinel = "typed-debug-private-sentinel";
        let request = TypedInvocationRequest::new(
            "http.get",
            serde_json::json!({"bearer": sentinel}),
            ExecutionContextWire::new("debug", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([("password".to_owned(), sentinel.to_owned())]),
            },
        )]));

        let rendered = format!("{request:?}");
        assert!(!rendered.contains(sentinel));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn noninteractive_children_remove_vault_master_key() {
        let command = noninteractive_command("unused");
        assert!(command.get_envs().any(|(name, value)| {
            name == OsStr::new(AH_VAULT_MASTER_KEY_ENV) && value.is_none()
        }));
    }

    #[cfg(windows)]
    #[test]
    fn noninteractive_child_has_no_console_window() {
        let output = noninteractive_command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                r#"Add-Type -Name NativeMethods -Namespace Win32 -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();'; [Win32.NativeMethods]::GetConsoleWindow().ToInt64()"#,
            ])
            .output()
            .expect("run console probe");

        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0");
    }

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
            cwd: None,
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
    fn secret_slot_serializes_in_command_descriptor() {
        let descriptor = sample_descriptor().with_secret_slot(SecretSlot::optional(
            "database",
            ["postgres"],
            "Database credential",
        ));

        let serialized = serde_json::to_value(descriptor).expect("descriptor should serialize");
        assert_eq!(serialized["secret_slots"][0]["name"], "database");
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

    impl BindResolvedSecrets for () {}

    #[test]
    fn plugin_parser_panic_becomes_structured_error() {
        let request = InvocationRequest::new("test", vec!["run".to_owned()], base_globals());
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
    fn plugin_executor_panic_does_not_poison_later_invocation() {
        let request = InvocationRequest::new("test", vec!["run".to_owned()], base_globals());
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
