use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    GlobalOptionsWire, Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use serde_json::{Value, json};

use super::*;

pub(super) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        PLUGIN_NAME,
        DOMAIN,
        vec![ask_descriptor(), chat_descriptor()],
    )
}

pub(super) fn invoke(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let command = match typed_command(request) {
        Ok(command) => command,
        Err(error) => return TypedInvocationResponse::error(error),
    };
    let globals = GlobalOptionsWire {
        json: true,
        quiet: false,
        limit: request.context.limit,
    };
    invocation_response(request, execute(OllamaCli { command }, &globals))
}

pub(super) fn cancel(_request_id: &str) -> bool {
    false
}

fn typed_command(request: &TypedInvocationRequest) -> Result<OllamaCommand, CommandError> {
    let cap = remaining_seconds(request);
    match request.command.as_str() {
        "ollama.ask" => {
            let mut args: AskArgs = decode(request)?;
            require_text(request, "model", &args.model)?;
            require_text(request, "prompt", &args.prompt)?;
            args.connection.timeout_secs = args.connection.timeout_secs.clamp(1, cap);
            Ok(OllamaCommand::Ask(args))
        }
        "ollama.chat" => {
            let mut args: ChatArgs = decode(request)?;
            require_text(request, "model", &args.model)?;
            require_text(request, "message", &args.message)?;
            args.connection.timeout_secs = args.connection.timeout_secs.clamp(1, cap);
            Ok(OllamaCommand::Chat(args))
        }
        _ => Err(command_error(
            request,
            "TYPED_COMMAND_NOT_FOUND",
            "Unknown Ollama command",
            "the command is not present in the Ollama typed catalog",
            false,
        )),
    }
}

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the type disagree.
fn decode<T: serde::de::DeserializeOwned>(
    request: &TypedInvocationRequest,
) -> Result<T, CommandError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Invalid arguments for {}", request.command),
            error.to_string(),
            false,
        )
    })
}

/// `minLength` rejects an empty string, but not one that is only whitespace.
fn require_text(
    request: &TypedInvocationRequest,
    name: &str,
    value: &str,
) -> Result<(), CommandError> {
    if value.trim().is_empty() {
        return Err(command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Missing {name}"),
            format!("typed input requires non-empty '{name}'"),
            false,
        ));
    }
    Ok(())
}

fn remaining_seconds(request: &TypedInvocationRequest) -> u64 {
    request
        .context
        .remaining_timeout_ms
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or(1)
        .max(1)
}

fn invocation_response(
    request: &TypedInvocationRequest,
    response: InvocationResponse,
) -> TypedInvocationResponse {
    if !response.success {
        if let Some(diagnostic) = response.diagnostic {
            return TypedInvocationResponse::error(CommandError::from_diagnostic(
                diagnostic
                    .with_domain(DOMAIN)
                    .with_operation(request.command.clone()),
                retryable_code(
                    response
                        .error_code
                        .as_deref()
                        .unwrap_or("OLLAMA_REQUEST_FAILED"),
                ),
            ));
        }
        let code = response
            .error_code
            .unwrap_or_else(|| "OLLAMA_REQUEST_FAILED".to_owned());
        let message = response
            .error_message
            .unwrap_or_else(|| "Ollama command failed".to_owned());
        return TypedInvocationResponse::error(command_error(
            request,
            &code,
            &message,
            &message,
            retryable_code(&code),
        ));
    }
    let Some(raw) = response.message else {
        return TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Ollama command returned no structured output",
            "the shared command implementation omitted its JSON result",
            false,
        ));
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(data) if data.is_object() => {
            TypedInvocationResponse::success(data, Some(format!("Completed {}.", request.command)))
        }
        Ok(_) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Ollama command returned non-object output",
            "typed commands require a JSON object result",
            false,
        )),
        Err(error) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Failed to decode Ollama command output",
            error.to_string(),
            false,
        )),
    }
}

fn retryable_code(code: &str) -> bool {
    code.contains("HTTP") || code.contains("TIMEOUT") || code.contains("API_FAILED")
}

fn command_error(
    request: &TypedInvocationRequest,
    code: impl Into<String>,
    message: impl Into<String>,
    cause: impl Into<String>,
    retryable: bool,
) -> CommandError {
    CommandError::new(
        Some(DOMAIN.to_owned()),
        Some(request.command.clone()),
        code,
        message,
        cause,
        1,
        retryable,
    )
}

fn ask_descriptor() -> CommandDescriptor {
    descriptor(
        "ollama.ask",
        "Generate with Ollama",
        "Generate one non-streaming response with Ollama /api/generate.",
        input_schema_for::<AskArgs>(),
        output_schema_for::<OllamaOutput>("ask"),
    )
    .with_example(CommandExample::new(
        "Summarize a concept",
        json!({
            "model": "llama3.2",
            "prompt": "Summarize Rust ownership in three bullets."
        }),
    ))
}

fn chat_descriptor() -> CommandDescriptor {
    descriptor(
        "ollama.chat",
        "Chat with Ollama",
        "Send one user message and optional system instruction to Ollama /api/chat.",
        input_schema_for::<ChatArgs>(),
        output_schema_for::<OllamaOutput>("chat"),
    )
    .with_example(CommandExample::new(
        "Ask for test cases",
        json!({
            "model": "llama3.2",
            "message": "Generate test names for parser edge cases."
        }),
    ))
}

fn descriptor(
    id: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
) -> CommandDescriptor {
    CommandDescriptor::new(
        id,
        title,
        description,
        input_schema,
        output_schema,
        CommandEffects::new(
            false,
            false,
            false,
            true,
            vec![
                CommandEffect::NetworkWrite,
                CommandEffect::ExternalWrite,
                CommandEffect::ConfigurationRead,
            ],
            RiskLevel::High,
            "Sends the prompt, system text, and model name to the configured base URL and consumes remote or local inference resources; transmission and compute usage cannot be undone.",
            Reversibility::No,
        ),
    )
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn catalog_contains_all_ollama_commands() {
        let catalog = command_catalog();
        assert_eq!(catalog.commands.len(), 2);
        assert!(catalog.commands.iter().all(|command| {
            command.input_schema["type"] == "object" && command.output_schema["type"] == "object"
        }));
        assert_eq!(catalog.commands[0].id, "ollama.ask");
        assert_eq!(catalog.commands[1].id, "ollama.chat");
    }

    #[test]
    fn typed_timeout_is_capped_by_request_deadline() {
        let request = TypedInvocationRequest::new(
            "ollama.ask",
            json!({
                "model": "llama3.2",
                "prompt": "hello",
                "timeout_secs": 120
            }),
            ExecutionContextWire::new("request-1", ".", None, 1_100),
        );

        let OllamaCommand::Ask(args) = typed_command(&request).expect("command should parse")
        else {
            panic!("expected ask command");
        };
        assert_eq!(args.connection.timeout_secs, 2);
    }
}
