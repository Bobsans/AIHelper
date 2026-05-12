use std::{
    ffi::c_char,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
    time::Duration,
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AhPluginApiV1, GlobalOptionsWire, InvocationRequest, InvocationResponse,
    ManualCommand, ManualExample, PluginManual, c_ptr_to_string, free_c_string_ptr,
    manual_to_c_string, response_to_c_string,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

const DOMAIN: &str = "ollama";
const PLUGIN_NAME: &str = "external-ollama";
const DESCRIPTION: &str = "Ollama Local API plugin (dynamic)";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

static PLUGIN_NAME_C: &[u8] = b"external-ollama\0";
static DOMAIN_C: &[u8] = b"ollama\0";
static DESCRIPTION_C: &[u8] = b"Ollama Local API plugin (dynamic)\0";

static PLUGIN_API_PTR: AtomicPtr<AhPluginApiV1> = AtomicPtr::new(ptr::null_mut());

#[derive(Debug, Parser)]
#[command(name = "ollama", about = "Ollama Local API commands")]
struct OllamaCli {
    #[command(subcommand)]
    command: OllamaCommand,
}

#[derive(Debug, Subcommand)]
enum OllamaCommand {
    #[command(about = "Single prompt generation using /api/generate")]
    Ask(AskArgs),
    #[command(about = "Single-message chat using /api/chat")]
    Chat(ChatArgs),
}

#[derive(Debug, Args)]
struct AskArgs {
    #[arg(
        long,
        value_name = "MODEL",
        help = "Ollama model name, for example llama3.2"
    )]
    model: String,
    #[arg(long, value_name = "TEXT", help = "Prompt text")]
    prompt: String,
    #[arg(long, value_name = "TEXT", help = "Optional system instruction")]
    system: Option<String>,
    #[command(flatten)]
    connection: ConnectionArgs,
}

#[derive(Debug, Args)]
struct ChatArgs {
    #[arg(
        long,
        value_name = "MODEL",
        help = "Ollama model name, for example llama3.2"
    )]
    model: String,
    #[arg(long, value_name = "TEXT", help = "User message text")]
    message: String,
    #[arg(long, value_name = "TEXT", help = "Optional system instruction")]
    system: Option<String>,
    #[command(flatten)]
    connection: ConnectionArgs,
}

#[derive(Debug, Args)]
struct ConnectionArgs {
    #[arg(
        long,
        value_name = "URL",
        default_value = DEFAULT_BASE_URL,
        help = "Ollama base URL"
    )]
    base_url: String,
    #[arg(
        long,
        value_name = "SECONDS",
        default_value_t = DEFAULT_TIMEOUT_SECS,
        help = "HTTP timeout in seconds"
    )]
    timeout_secs: u64,
}

#[derive(Debug, Serialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessageRequest>,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct ChatMessageRequest {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct GenerateResponse {
    model: Option<String>,
    response: Option<String>,
    done: Option<bool>,
    done_reason: Option<String>,
    created_at: Option<String>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    prompt_eval_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    model: Option<String>,
    message: Option<ChatMessageResponse>,
    done: Option<bool>,
    done_reason: Option<String>,
    created_at: Option<String>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    prompt_eval_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct OllamaOutput {
    command: String,
    model: String,
    response: String,
    done: Option<bool>,
    done_reason: Option<String>,
    created_at: Option<String>,
    metrics: OllamaMetrics,
}

#[derive(Debug, Serialize)]
struct OllamaMetrics {
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
    prompt_eval_duration: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

/// Returns the Ollama plugin ABI entry point.
///
/// # Safety
///
/// The returned pointer is process-static and must not be freed or mutated by
/// the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_entry_v1() -> *const AhPluginApiV1 {
    let existing = PLUGIN_API_PTR.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing.cast_const();
    }

    let created = Box::into_raw(Box::new(AhPluginApiV1 {
        abi_version: AH_PLUGIN_ABI_VERSION,
        plugin_name: PLUGIN_NAME_C.as_ptr().cast(),
        domain: DOMAIN_C.as_ptr().cast(),
        description: DESCRIPTION_C.as_ptr().cast(),
        invoke_json: ah_plugin_invoke_json,
        free_c_string: ah_plugin_free_c_string,
    }));

    match PLUGIN_API_PTR.compare_exchange(
        ptr::null_mut(),
        created,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => created.cast_const(),
        Err(existing) => {
            unsafe { drop(Box::from_raw(created)) };
            existing.cast_const()
        }
    }
}

/// Returns the Ollama plugin manual JSON as an owned C string.
///
/// # Safety
///
/// The caller must free the returned pointer with this plugin's
/// `free_c_string` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_manual_json_v1() -> *mut c_char {
    manual_to_c_string(&plugin_manual())
}

unsafe extern "C" fn ah_plugin_invoke_json(request_json: *const c_char) -> *mut c_char {
    let response = invoke_from_raw(request_json);
    response_to_c_string(&response)
}

unsafe extern "C" fn ah_plugin_free_c_string(value: *mut c_char) {
    unsafe { free_c_string_ptr(value) };
}

fn invoke_from_raw(request_json: *const c_char) -> InvocationResponse {
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

    if request.domain != DOMAIN {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!(
                "plugin domain mismatch: expected '{DOMAIN}', got '{}'",
                request.domain
            ),
        );
    }

    let parsed = match parse_args(&request.argv) {
        Ok(value) => value,
        Err(response) => return response,
    };

    match parsed.command {
        OllamaCommand::Ask(args) => execute_ask(args, &request.globals),
        OllamaCommand::Chat(args) => execute_chat(args, &request.globals),
    }
}

fn parse_args(argv: &[String]) -> Result<OllamaCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match OllamaCli::try_parse_from(args) {
        Ok(value) => Ok(value),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                Err(InvocationResponse::ok(Some(error.to_string())))
            } else {
                Err(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    error.to_string(),
                ))
            }
        }
    }
}

fn execute_ask(args: AskArgs, globals: &GlobalOptionsWire) -> InvocationResponse {
    let AskArgs {
        model,
        prompt,
        system,
        connection,
    } = args;

    let request = GenerateRequest {
        model: model.clone(),
        prompt,
        system,
        stream: false,
    };

    let response = match ollama_post::<GenerateRequest, GenerateResponse>(
        &connection.base_url,
        "/api/generate",
        &request,
        connection.timeout_secs,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };

    let text = match non_empty_response_text(response.response) {
        Ok(value) => value,
        Err(error) => return error,
    };

    let output = OllamaOutput {
        command: "ask".to_owned(),
        model: response.model.unwrap_or(model),
        response: text.clone(),
        done: response.done,
        done_reason: response.done_reason,
        created_at: response.created_at,
        metrics: OllamaMetrics {
            total_duration: response.total_duration,
            load_duration: response.load_duration,
            prompt_eval_count: response.prompt_eval_count,
            prompt_eval_duration: response.prompt_eval_duration,
            eval_count: response.eval_count,
            eval_duration: response.eval_duration,
        },
    };

    render_success(globals, &output, text)
}

fn execute_chat(args: ChatArgs, globals: &GlobalOptionsWire) -> InvocationResponse {
    let ChatArgs {
        model,
        message,
        system,
        connection,
    } = args;

    let mut messages = Vec::new();
    if let Some(system_text) = system {
        messages.push(ChatMessageRequest {
            role: "system".to_owned(),
            content: system_text,
        });
    }
    messages.push(ChatMessageRequest {
        role: "user".to_owned(),
        content: message,
    });

    let request = ChatRequest {
        model: model.clone(),
        messages,
        stream: false,
    };

    let response = match ollama_post::<ChatRequest, ChatResponse>(
        &connection.base_url,
        "/api/chat",
        &request,
        connection.timeout_secs,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };

    let text = match response.message.and_then(|item| item.content) {
        Some(value) if !value.trim().is_empty() => value,
        _ => {
            return InvocationResponse::error(
                "OLLAMA_RESPONSE_INVALID",
                "ollama chat response has empty message content",
            );
        }
    };

    let output = OllamaOutput {
        command: "chat".to_owned(),
        model: response.model.unwrap_or(model),
        response: text.clone(),
        done: response.done,
        done_reason: response.done_reason,
        created_at: response.created_at,
        metrics: OllamaMetrics {
            total_duration: response.total_duration,
            load_duration: response.load_duration,
            prompt_eval_count: response.prompt_eval_count,
            prompt_eval_duration: response.prompt_eval_duration,
            eval_count: response.eval_count,
            eval_duration: response.eval_duration,
        },
    };

    render_success(globals, &output, text)
}

fn render_success(
    globals: &GlobalOptionsWire,
    output: &OllamaOutput,
    text_output: String,
) -> InvocationResponse {
    if globals.quiet {
        return InvocationResponse::ok(None);
    }
    if globals.json {
        match serde_json::to_string_pretty(output) {
            Ok(payload) => InvocationResponse::ok(Some(payload)),
            Err(error) => InvocationResponse::error(
                "JSON_SERIALIZATION_FAILED",
                format!("failed to serialize plugin output: {error}"),
            ),
        }
    } else {
        InvocationResponse::ok(Some(text_output))
    }
}

fn non_empty_response_text(value: Option<String>) -> Result<String, InvocationResponse> {
    match value {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(InvocationResponse::error(
            "OLLAMA_RESPONSE_INVALID",
            "ollama generate response has empty 'response' field",
        )),
    }
}

fn ollama_post<TRequest, TResponse>(
    base_url: &str,
    path: &str,
    request: &TRequest,
    timeout_secs: u64,
) -> Result<TResponse, InvocationResponse>
where
    TRequest: Serialize,
    TResponse: DeserializeOwned,
{
    let base_url = normalize_base_url(base_url)?;
    let url = format!("{base_url}{path}");

    let client = Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(1)))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "OLLAMA_HTTP_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;

    let response = client.post(&url).json(request).send().map_err(|error| {
        InvocationResponse::error(
            "OLLAMA_HTTP_FAILED",
            format!("request to '{url}' failed: {error}"),
        )
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "<failed to read response body>".to_owned());
        return Err(InvocationResponse::error(
            "OLLAMA_API_FAILED",
            format!(
                "ollama returned HTTP {status} for '{url}': {}",
                truncate_for_error(&body, 400)
            ),
        ));
    }

    response.json::<TResponse>().map_err(|error| {
        InvocationResponse::error(
            "OLLAMA_RESPONSE_INVALID",
            format!("failed to decode response from '{url}': {error}"),
        )
    })
}

fn normalize_base_url(base_url: &str) -> Result<String, InvocationResponse> {
    let normalized = base_url.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--base-url must not be empty",
        ));
    }
    Ok(normalized)
}

fn truncate_for_error(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect::<String>() + "..."
}

fn plugin_manual() -> PluginManual {
    PluginManual {
        plugin_name: PLUGIN_NAME.to_owned(),
        domain: DOMAIN.to_owned(),
        description: DESCRIPTION.to_owned(),
        commands: vec![
            ManualCommand {
                name: "ask".to_owned(),
                summary: "Single prompt generation via Ollama /api/generate.".to_owned(),
                usage: "ask --model <MODEL> --prompt <TEXT> [--system <TEXT>] [--base-url <URL>] [--timeout-secs <SECONDS>]".to_owned(),
                examples: vec![
                    manual_example(
                        "Minimal prompt",
                        &["ask", "--model", "llama3.2", "--prompt", "Summarize Rust ownership in 3 bullets"],
                    ),
                    manual_example(
                        "Prompt with system instruction",
                        &[
                            "ask",
                            "--model",
                            "qwen2.5-coder",
                            "--system",
                            "You are a terse senior engineer",
                            "--prompt",
                            "Propose git commit message for staged changes",
                        ],
                    ),
                ],
            },
            ManualCommand {
                name: "chat".to_owned(),
                summary: "Single message chat completion via Ollama /api/chat.".to_owned(),
                usage: "chat --model <MODEL> --message <TEXT> [--system <TEXT>] [--base-url <URL>] [--timeout-secs <SECONDS>]".to_owned(),
                examples: vec![
                    manual_example(
                        "One-shot chat message",
                        &[
                            "chat",
                            "--model",
                            "llama3.2",
                            "--message",
                            "Generate test names for file parser edge cases",
                        ],
                    ),
                    manual_example(
                        "Chat with explicit base URL",
                        &[
                            "chat",
                            "--model",
                            "mistral",
                            "--base-url",
                            "http://127.0.0.1:11434",
                            "--message",
                            "List 5 refactoring steps for long functions",
                        ],
                    ),
                ],
            },
        ],
        notes: vec![
            "Requires running Ollama server (default: http://127.0.0.1:11434).".to_owned(),
            "Use global --json for structured machine-readable output.".to_owned(),
            "Plugin never streams responses; it waits for final message.".to_owned(),
        ],
    }
}

fn manual_example(description: &str, argv: &[&str]) -> ManualExample {
    ManualExample {
        description: description.to_owned(),
        argv: argv.iter().map(|item| (*item).to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant},
    };

    use clap::{CommandFactory, Parser};
    use serde_json::Value;

    use super::*;

    #[test]
    fn manual_examples_parse() {
        let manual = plugin_manual();
        for command in &manual.commands {
            for example in &command.examples {
                let mut args = Vec::with_capacity(example.argv.len() + 1);
                args.push(manual.domain.clone());
                args.extend(example.argv.iter().cloned());
                let parse_result = OllamaCli::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for command '{}': argv={args:?}",
                    command.name
                );
            }
        }
    }

    #[test]
    fn base_url_is_normalized() {
        let normalized =
            normalize_base_url(" http://127.0.0.1:11434/ ").expect("base url should normalize");
        assert_eq!(normalized, "http://127.0.0.1:11434");
    }

    #[test]
    fn empty_base_url_is_rejected() {
        let result = normalize_base_url("   ");
        assert!(result.is_err());
    }

    #[test]
    fn parser_builds_command_tree() {
        let _ = OllamaCli::command();
    }

    #[test]
    fn ask_posts_generate_request_and_returns_json_output() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "model": "llama3.2",
                "response": "summary text",
                "done": true,
                "done_reason": "stop",
                "created_at": "2026-05-06T00:00:00Z",
                "total_duration": 10,
                "load_duration": 2,
                "prompt_eval_count": 3,
                "prompt_eval_duration": 4,
                "eval_count": 5,
                "eval_duration": 6
            }"#,
        )]);

        let response = invoke_json(&[
            "ask",
            "--model",
            "llama3.2",
            "--prompt",
            "summarize",
            "--system",
            "be terse",
            "--base-url",
            &server.url(),
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "ask");
        assert_eq!(payload["model"], "llama3.2");
        assert_eq!(payload["response"], "summary text");
        assert_eq!(payload["metrics"]["eval_count"], 5);

        let request = only_request(&server);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/generate");
        assert_eq!(request.header("content-type"), Some("application/json"));
        let body: Value = serde_json::from_str(&request.body).expect("body should be json");
        assert_eq!(body["model"], "llama3.2");
        assert_eq!(body["prompt"], "summarize");
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["stream"], false);
    }

    #[test]
    fn ask_text_mode_returns_plain_response() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "model": "llama3.2",
                "response": "plain text",
                "done": true
            }"#,
        )]);

        let response = invoke_text(&[
            "ask",
            "--model",
            "llama3.2",
            "--prompt",
            "say hi",
            "--base-url",
            &server.url(),
        ]);

        assert!(response.success, "{response:?}");
        assert_eq!(response.message.as_deref(), Some("plain text"));
    }

    #[test]
    fn chat_posts_messages_and_returns_json_output() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "model": "mistral",
                "message": { "content": "chat answer" },
                "done": true,
                "done_reason": "stop",
                "created_at": "2026-05-06T00:00:00Z",
                "total_duration": 100,
                "load_duration": 20,
                "prompt_eval_count": 30,
                "prompt_eval_duration": 40,
                "eval_count": 50,
                "eval_duration": 60
            }"#,
        )]);

        let response = invoke_json(&[
            "chat",
            "--model",
            "mistral",
            "--system",
            "system prompt",
            "--message",
            "user message",
            "--base-url",
            &server.url(),
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "chat");
        assert_eq!(payload["response"], "chat answer");
        assert_eq!(payload["metrics"]["total_duration"], 100);

        let request = only_request(&server);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/api/chat");
        let body: Value = serde_json::from_str(&request.body).expect("body should be json");
        assert_eq!(body["model"], "mistral");
        assert_eq!(body["stream"], false);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "system prompt");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "user message");
    }

    #[test]
    fn http_error_has_stable_error_code_and_truncated_body() {
        let long_body = "x".repeat(500);
        let server = MockServer::new(vec![MockResponse::text(500, &long_body)]);

        let response = invoke_json(&[
            "ask",
            "--model",
            "llama3.2",
            "--prompt",
            "fail",
            "--base-url",
            &server.url(),
        ]);

        assert!(!response.success);
        assert_eq!(response.error_code.as_deref(), Some("OLLAMA_API_FAILED"));
        let message = response.error_message.as_deref().unwrap_or("");
        assert!(message.contains("HTTP 500"));
        assert!(message.ends_with("..."));
        assert!(message.len() < long_body.len() + 200);
    }

    #[test]
    fn invalid_json_response_has_stable_error_code() {
        let server = MockServer::new(vec![MockResponse::text(200, "not-json")]);

        let response = invoke_json(&[
            "ask",
            "--model",
            "llama3.2",
            "--prompt",
            "decode",
            "--base-url",
            &server.url(),
        ]);

        assert!(!response.success);
        assert_eq!(
            response.error_code.as_deref(),
            Some("OLLAMA_RESPONSE_INVALID")
        );
    }

    #[test]
    fn empty_generate_response_is_rejected() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{"model":"llama3.2","response":"   ","done":true}"#,
        )]);

        let response = invoke_json(&[
            "ask",
            "--model",
            "llama3.2",
            "--prompt",
            "empty",
            "--base-url",
            &server.url(),
        ]);

        assert!(!response.success);
        assert_eq!(
            response.error_code.as_deref(),
            Some("OLLAMA_RESPONSE_INVALID")
        );
        assert!(
            response
                .error_message
                .as_deref()
                .unwrap_or("")
                .contains("empty 'response' field")
        );
    }

    #[test]
    fn empty_chat_message_is_rejected() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{"model":"mistral","message":{"content":""},"done":true}"#,
        )]);

        let response = invoke_json(&[
            "chat",
            "--model",
            "mistral",
            "--message",
            "empty",
            "--base-url",
            &server.url(),
        ]);

        assert!(!response.success);
        assert_eq!(
            response.error_code.as_deref(),
            Some("OLLAMA_RESPONSE_INVALID")
        );
        assert!(
            response
                .error_message
                .as_deref()
                .unwrap_or("")
                .contains("empty message content")
        );
    }

    #[test]
    fn quiet_mode_suppresses_success_message() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "model": "llama3.2",
                "response": "hidden",
                "done": true
            }"#,
        )]);

        let response = invoke_with_globals(
            &[
                "ask",
                "--model",
                "llama3.2",
                "--prompt",
                "quiet",
                "--base-url",
                &server.url(),
            ],
            GlobalOptionsWire {
                json: false,
                quiet: true,
                limit: None,
            },
        );

        assert!(response.success, "{response:?}");
        assert_eq!(response.message, None);
    }

    fn invoke_json(argv: &[&str]) -> InvocationResponse {
        invoke_with_globals(
            argv,
            GlobalOptionsWire {
                json: true,
                quiet: false,
                limit: None,
            },
        )
    }

    fn invoke_text(argv: &[&str]) -> InvocationResponse {
        invoke_with_globals(
            argv,
            GlobalOptionsWire {
                json: false,
                quiet: false,
                limit: None,
            },
        )
    }

    fn invoke_with_globals(argv: &[&str], globals: GlobalOptionsWire) -> InvocationResponse {
        let request = InvocationRequest {
            domain: DOMAIN.to_owned(),
            argv: argv.iter().map(|item| (*item).to_owned()).collect(),
            globals,
        };
        let request_json = serde_json::to_string(&request).expect("request should serialize");
        let request_c = std::ffi::CString::new(request_json).expect("request should be cstring");
        invoke_from_raw(request_c.as_ptr())
    }

    fn response_json(response: &InvocationResponse) -> Value {
        serde_json::from_str(response.message.as_deref().expect("message should exist"))
            .expect("message should be json")
    }

    fn only_request(server: &MockServer) -> CapturedRequest {
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        requests[0].clone()
    }

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: String,
    }

    impl CapturedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .get(&name.to_ascii_lowercase())
                .map(String::as_str)
        }
    }

    struct MockResponse {
        status: u16,
        content_type: &'static str,
        body: String,
    }

    impl MockResponse {
        fn json(status: u16, body: &str) -> Self {
            Self {
                status,
                content_type: "application/json",
                body: body.to_owned(),
            }
        }

        fn text(status: u16, body: &str) -> Self {
            Self {
                status,
                content_type: "text/plain",
                body: body.to_owned(),
            }
        }
    }

    struct MockServer {
        url: String,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn new(responses: Vec<MockResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("mock server should bind");
            listener
                .set_nonblocking(true)
                .expect("listener should be nonblocking");
            let url = format!(
                "http://{}",
                listener.local_addr().expect("local addr should exist")
            );
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&requests);
            let handle = thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                for response in responses {
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                handle_connection(stream, response, &captured);
                                break;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                if Instant::now() > deadline {
                                    return;
                                }
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => return,
                        }
                    }
                }
            });

            Self {
                url,
                requests,
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            self.url.clone()
        }

        fn requests(&self) -> Vec<CapturedRequest> {
            if let Some(handle) = &self.handle {
                while !handle.is_finished() {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            self.requests.lock().expect("requests lock").clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_connection(
        mut stream: TcpStream,
        response: MockResponse,
        requests: &Arc<Mutex<Vec<CapturedRequest>>>,
    ) {
        let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));
        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .expect("request line should read");
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_owned();
        let path = parts.next().unwrap_or("").to_owned();

        let mut headers = HashMap::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header should read");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                let key = name.trim().to_ascii_lowercase();
                let value = value.trim().to_owned();
                if key == "content-length" {
                    content_length = value.parse::<usize>().unwrap_or(0);
                }
                headers.insert(key, value);
            }
        }

        let mut body_bytes = vec![0; content_length];
        if content_length > 0 {
            reader
                .read_exact(&mut body_bytes)
                .expect("request body should read");
        }
        let body = String::from_utf8_lossy(&body_bytes).into_owned();
        requests
            .lock()
            .expect("requests lock")
            .push(CapturedRequest {
                method,
                path,
                headers,
                body,
            });

        let reason = match response.status {
            200 => "OK",
            500 => "Internal Server Error",
            _ => "OK",
        };
        let response_headers = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            reason,
            response.content_type,
            response.body.len()
        );
        stream
            .write_all(response_headers.as_bytes())
            .expect("response headers should write");
        stream
            .write_all(response.body.as_bytes())
            .expect("response body should write");
    }
}
