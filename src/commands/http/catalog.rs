//! What `http` publishes to the plugin catalog: one descriptor per command,
//! with its declared effects, risk and secret slot.

use super::*;

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        "builtin-http",
        "http",
        vec![
            request_descriptor(),
            shortcut_descriptor("get", "GET", true),
            shortcut_descriptor("post", "POST", false),
            shortcut_descriptor("put", "PUT", false),
            shortcut_descriptor("patch", "PATCH", false),
            shortcut_descriptor("delete", "DELETE", false),
            replay_descriptor(),
            assert_descriptor("assert"),
            assert_descriptor("run"),
        ],
    )
}

pub(super) fn request_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "http.request",
        "Send HTTP request",
        "Send an HTTP request with explicit method, payload, authentication, and expectations.",
        input_schema_for::<RequestWireArgs>(),
        output_schema_for::<domain::HttpRequestOutput>("http.request"),
        http_write_effects(
            "Sends an arbitrary HTTP method and optional credentials or payload to an arbitrary URL; the remote service may mutate state.",
        ),
    )
    .with_secret_slot(http_basic_slot())
}

pub(super) fn shortcut_descriptor(
    command: &str,
    method: &str,
    read_only: bool,
) -> CommandDescriptor {
    let descriptor = CommandDescriptor::new(
        format!("http.{command}"),
        format!("Send HTTP {method}"),
        format!("Send an HTTP {method} request with payload, authentication, and expectations."),
        input_schema_for::<MethodShortcutWireArgs>(),
        output_schema_for::<domain::HttpRequestOutput>(&format!("http.{command}")),
        if read_only {
            http_read_effects(
                "Sends an HTTP GET request and optional credentials to an arbitrary URL; servers can still implement side effects for GET.",
            )
        } else {
            http_write_effects(&format!(
                "Sends an HTTP {method} request and optional credentials or payload to an arbitrary URL; the remote service may mutate state."
            ))
        },
    );
    if matches!(command, "get" | "post") {
        descriptor.with_secret_slot(http_basic_slot())
    } else {
        descriptor
    }
}

pub(super) fn replay_descriptor() -> CommandDescriptor {
    CommandDescriptor::new(
        "http.replay",
        "Replay curl request",
        "Parse and replay a supported curl command with optional expectation overrides.",
        input_schema_for::<ReplayWireArgs>(),
        output_schema_for::<domain::HttpRequestOutput>("http.replay"),
        http_write_effects(
            "Replays an arbitrary HTTP request encoded in curl syntax and may send embedded credentials or mutate a remote service.",
        ),
    )
    .with_secret_slot(http_basic_slot())
}

pub(super) fn http_basic_slot() -> SecretSlot {
    SecretSlot::optional("basic", ["http-basic"], "HTTP Basic credential.")
}

pub(super) fn assert_descriptor(command: &str) -> CommandDescriptor {
    CommandDescriptor::new(
        format!("http.{command}"),
        if command == "assert" {
            "Run HTTP assertions"
        } else {
            "Run HTTP assertion alias"
        },
        "Execute all HTTP cases in a YAML or JSON assertion spec.",
        input_schema_for::<AssertWireArgs>(),
        output_schema_for::<domain::HttpAssertOutput>("http.assert"),
        CommandEffects::new(
            false,
            false,
            false,
            true,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::NetworkRead,
                CommandEffect::NetworkWrite,
                CommandEffect::ExternalRead,
                CommandEffect::ExternalWrite,
            ],
            RiskLevel::High,
            "Reads a local spec and payload files, then sends every declared HTTP method, credential, and payload to declared URLs; cases may mutate external systems.",
            Reversibility::Unknown,
        ),
    )
}

pub(super) fn http_read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        true,
        vec![
            CommandEffect::NetworkRead,
            CommandEffect::ExternalRead,
            CommandEffect::FilesystemRead,
        ],
        RiskLevel::Medium,
        impact,
        Reversibility::Unknown,
    )
}

pub(super) fn http_write_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        false,
        false,
        false,
        true,
        vec![
            CommandEffect::NetworkRead,
            CommandEffect::NetworkWrite,
            CommandEffect::ExternalRead,
            CommandEffect::ExternalWrite,
            CommandEffect::FilesystemRead,
        ],
        RiskLevel::High,
        impact,
        Reversibility::Unknown,
    )
}
