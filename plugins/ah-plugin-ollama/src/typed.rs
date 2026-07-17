use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    GlobalOptionsWire, Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use serde_json::{Map, Value, json};

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
    let arguments = &request.arguments;
    let connection = ConnectionArgs {
        base_url: string_or(arguments, "base_url", DEFAULT_BASE_URL),
        timeout_secs: u64_or(arguments, "timeout_secs", DEFAULT_TIMEOUT_SECS)
            .min(remaining_seconds(request)),
    };
    match request.command.as_str() {
        "ollama.ask" => Ok(OllamaCommand::Ask(AskArgs {
            model: required_string(arguments, "model", request)?,
            prompt: required_string(arguments, "prompt", request)?,
            system: optional_string(arguments, "system"),
            connection,
        })),
        "ollama.chat" => Ok(OllamaCommand::Chat(ChatArgs {
            model: required_string(arguments, "model", request)?,
            message: required_string(arguments, "message", request)?,
            system: optional_string(arguments, "system"),
            connection,
        })),
        _ => Err(command_error(
            request,
            "TYPED_COMMAND_NOT_FOUND",
            "Unknown Ollama command",
            "the command is not present in the Ollama typed catalog",
            false,
        )),
    }
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

fn required_string(
    arguments: &Value,
    name: &str,
    request: &TypedInvocationRequest,
) -> Result<String, CommandError> {
    optional_string(arguments, name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            command_error(
                request,
                "INVALID_ARGUMENT",
                format!("Missing {name}"),
                format!("typed input requires non-empty '{name}'"),
                false,
            )
        })
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn string_or(arguments: &Value, name: &str, default: &str) -> String {
    optional_string(arguments, name).unwrap_or_else(|| default.to_owned())
}

fn u64_or(arguments: &Value, name: &str, default: u64) -> u64 {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(default)
        .max(1)
}

fn ask_descriptor() -> CommandDescriptor {
    descriptor(
        "ollama.ask",
        "Generate with Ollama",
        "Generate one non-streaming response with Ollama /api/generate.",
        prompt_input("prompt", "Prompt text."),
        output_schema("ask"),
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
        prompt_input("message", "User message text."),
        output_schema("chat"),
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

fn prompt_input(content_field: &str, content_description: &str) -> Value {
    let mut properties = Map::new();
    properties.insert(
        "model".to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "description": "Ollama model name."
        }),
    );
    properties.insert(
        content_field.to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "description": content_description
        }),
    );
    properties.insert(
        "system".to_owned(),
        json!({
            "type": "string",
            "description": "Optional system instruction sent to the model."
        }),
    );
    properties.insert(
        "base_url".to_owned(),
        json!({
            "type": "string",
            "minLength": 1,
            "default": DEFAULT_BASE_URL,
            "description": "Ollama base URL. Prompt data is sent to this address."
        }),
    );
    properties.insert(
        "timeout_secs".to_owned(),
        json!({
            "type": "integer",
            "minimum": 1,
            "default": DEFAULT_TIMEOUT_SECS,
            "description": "HTTP timeout, capped by the MCP request deadline."
        }),
    );
    json!({
        "type": "object",
        "properties": properties,
        "required": ["model", content_field],
        "additionalProperties": false
    })
}

fn output_schema(command: &str) -> Value {
    let nullable_string = json!({"type": ["string", "null"]});
    let nullable_boolean = json!({"type": ["boolean", "null"]});
    let nullable_integer = json!({"type": ["integer", "null"], "minimum": 0});
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": command},
            "model": {"type": "string"},
            "response": {"type": "string"},
            "done": nullable_boolean,
            "done_reason": nullable_string,
            "created_at": nullable_string,
            "metrics": {
                "type": "object",
                "properties": {
                    "total_duration": nullable_integer,
                    "load_duration": nullable_integer,
                    "prompt_eval_count": nullable_integer,
                    "prompt_eval_duration": nullable_integer,
                    "eval_count": nullable_integer,
                    "eval_duration": nullable_integer
                },
                "required": [
                    "total_duration",
                    "load_duration",
                    "prompt_eval_count",
                    "prompt_eval_duration",
                    "eval_count",
                    "eval_duration"
                ],
                "additionalProperties": false
            }
        },
        "required": [
            "command",
            "model",
            "response",
            "done",
            "done_reason",
            "created_at",
            "metrics"
        ],
        "additionalProperties": false
    })
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
