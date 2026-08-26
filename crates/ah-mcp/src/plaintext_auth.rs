//! Rejecting and redacting credentials a caller put in tool arguments in the
//! clear.
//!
//! MCP arguments are logged and echoed back, so a token pasted into one has to
//! be refused before it travels, and scrubbed from anything already captured.

use ah_plugin_api::CommandError;
use ah_redact::{REDACTED, curl_contains_auth, is_authorization_header, url_contains_userinfo};
use rmcp::model::JsonObject;
use serde_json::Value;

pub(crate) fn validate_mcp_plaintext_auth(
    command: &str,
    arguments: &JsonObject,
) -> Result<(), CommandError> {
    if let Some(domain) = forge_token_domain(command) {
        return if arguments.contains_key("token") {
            Err(mcp_plaintext_token_error(domain))
        } else {
            Ok(())
        };
    }
    if !command.starts_with("http.") {
        return Ok(());
    }
    if arguments.contains_key("bearer") || arguments.contains_key("basic") {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("headers")
        .and_then(Value::as_array)
        .is_some_and(|headers| {
            headers
                .iter()
                .filter_map(Value::as_str)
                .any(is_authorization_header)
        })
    {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(url_contains_userinfo)
    {
        return Err(mcp_plaintext_auth_error());
    }
    if arguments
        .get("curl")
        .and_then(Value::as_str)
        .is_some_and(curl_contains_auth)
    {
        return Err(mcp_plaintext_auth_error());
    }
    Ok(())
}

/// GitHub and GitLab carry one opaque token argument; over MCP it must come from
/// the vault instead of riding along in the tool call.
pub(crate) fn forge_token_domain(command: &str) -> Option<&'static str> {
    if command.starts_with("github.") {
        Some("github")
    } else if command.starts_with("gitlab.") {
        Some("gitlab")
    } else {
        None
    }
}

pub(crate) fn mcp_plaintext_token_error(domain: &'static str) -> CommandError {
    CommandError::new(
        Some(domain.to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Inline API tokens are not accepted over MCP",
        format!(
            "Store the token in the AH vault and pass its id through credentials.token, or let AIHelper use the host-bound {domain} environment variables"
        ),
        2,
        false,
    )
}

pub(crate) fn mcp_plaintext_auth_error() -> CommandError {
    CommandError::new(
        Some("http".to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Inline HTTP credentials are not accepted over MCP",
        "Store the credential in the AH vault and pass its id through credentials.basic",
        2,
        false,
    )
}

pub(crate) fn redact_mcp_plaintext_auth(arguments: &mut JsonObject) {
    for name in ["bearer", "basic", "token"] {
        if arguments.contains_key(name) {
            arguments.insert(name.to_owned(), Value::String(REDACTED.to_owned()));
        }
    }
    if let Some(Value::Array(headers)) = arguments.get_mut("headers") {
        for header in headers {
            if header.as_str().is_some_and(is_authorization_header) {
                *header = Value::String(format!("Authorization: {REDACTED}"));
            }
        }
    }
    if let Some(Value::String(url)) = arguments.get_mut("url")
        && url_contains_userinfo(url)
    {
        *url = REDACTED.to_owned();
    }
    if let Some(Value::String(curl)) = arguments.get_mut("curl")
        && curl_contains_auth(curl)
    {
        *curl = REDACTED.to_owned();
    }
    if let Some(Value::Object(nested)) = arguments.get_mut("arguments") {
        redact_mcp_plaintext_auth(nested);
    }
}
