use std::path::PathBuf;

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, Reversibility,
    RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use clap::{Args, Subcommand, ValueEnum};
use serde_json::{Map, Value, json};

use crate::{cli::GlobalOptions, error::AppError};

mod adapters {
    pub mod io;
    pub mod output;
}

mod domain;

#[derive(Debug, Args)]
pub struct HttpArgs {
    #[command(subcommand)]
    pub command: HttpCommand,
}

#[derive(Debug, Subcommand)]
pub enum HttpCommand {
    #[command(about = "Send HTTP request with explicit method")]
    Request(RequestArgs),
    #[command(about = "Send HTTP GET request")]
    Get(MethodShortcutArgs),
    #[command(about = "Send HTTP POST request")]
    Post(MethodShortcutArgs),
    #[command(about = "Send HTTP PUT request")]
    Put(MethodShortcutArgs),
    #[command(about = "Send HTTP PATCH request")]
    Patch(MethodShortcutArgs),
    #[command(about = "Send HTTP DELETE request")]
    Delete(MethodShortcutArgs),
    #[command(about = "Replay curl command through stable CLI contract")]
    Replay(ReplayArgs),
    #[command(about = "Run API assertions from spec file")]
    Assert(AssertArgs),
    #[command(about = "Alias for assert")]
    Run(AssertArgs),
}

#[derive(Debug, Args)]
pub struct RequestArgs {
    #[arg(long, value_name = "METHOD")]
    pub method: String,
    pub url: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct MethodShortcutArgs {
    pub url: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct ReplayArgs {
    #[arg(long, value_name = "CURL", help = "curl command to replay")]
    pub curl: String,
    #[command(flatten)]
    pub request: RequestOptionsArgs,
    #[command(flatten)]
    pub expect: RequestExpectArgs,
}

#[derive(Debug, Args)]
pub struct AssertArgs {
    pub spec_path: PathBuf,
    #[arg(long = "var", value_name = "KEY=VALUE")]
    pub vars: Vec<String>,
    #[arg(long)]
    pub fail_fast: bool,
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub report: Option<AssertReportArg>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum AssertReportArg {
    Text,
    Json,
    Junit,
}

#[derive(Debug, Args, Clone)]
pub struct RequestOptionsArgs {
    #[arg(long = "header", value_name = "K: V")]
    pub headers: Vec<String>,
    #[arg(long = "query", value_name = "KEY=VALUE")]
    pub query: Vec<String>,
    #[arg(long, value_name = "SECONDS")]
    pub timeout_secs: Option<u64>,
    #[arg(long, value_name = "BYTES")]
    pub max_response_bytes: Option<usize>,
    #[arg(long, value_name = "TOKEN")]
    pub bearer: Option<String>,
    #[arg(long, value_name = "USER:PASS")]
    pub basic: Option<String>,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub json_file: Option<PathBuf>,
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, Default)]
pub struct RequestExpectArgs {
    #[arg(long = "expect-status", value_name = "CODE_OR_RANGE")]
    pub expect_status: Option<String>,
    #[arg(long = "expect-header", value_name = "K: V")]
    pub expect_headers: Vec<String>,
    #[arg(long = "expect-body-contains", value_name = "TEXT")]
    pub expect_body_contains: Vec<String>,
    #[arg(
        long = "expect-json",
        value_name = "PATH:OP[:VALUE]",
        help = "JSON expectation expression, for example status:eq:ok"
    )]
    pub expect_json: Vec<String>,
}

pub fn execute(args: HttpArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        HttpCommand::Request(request_args) => execute_request(
            domain::run_request_command(request_args, "request"),
            options,
        ),
        HttpCommand::Get(method_args) => execute_shortcut("get", "GET", method_args, options),
        HttpCommand::Post(method_args) => execute_shortcut("post", "POST", method_args, options),
        HttpCommand::Put(method_args) => execute_shortcut("put", "PUT", method_args, options),
        HttpCommand::Patch(method_args) => execute_shortcut("patch", "PATCH", method_args, options),
        HttpCommand::Delete(method_args) => {
            execute_shortcut("delete", "DELETE", method_args, options)
        }
        HttpCommand::Replay(replay_args) => {
            execute_request(domain::run_replay(replay_args, "replay"), options)
        }
        HttpCommand::Assert(assert_args) => execute_assert(assert_args, options, "assert"),
        HttpCommand::Run(assert_args) => execute_assert(assert_args, options, "run"),
    }
}

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

pub(crate) fn invoke_typed(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let result = match request.command.as_str() {
        "http.request" => typed_request(request, "request", None),
        "http.get" => typed_request(request, "get", Some("GET")),
        "http.post" => typed_request(request, "post", Some("POST")),
        "http.put" => typed_request(request, "put", Some("PUT")),
        "http.patch" => typed_request(request, "patch", Some("PATCH")),
        "http.delete" => typed_request(request, "delete", Some("DELETE")),
        "http.replay" => typed_replay(request),
        "http.assert" => typed_assert(request, "assert"),
        "http.run" => typed_assert(request, "run"),
        _ => Err(AppError::invalid_argument(format!(
            "unknown typed HTTP command: {}",
            request.command
        ))),
    };
    match result {
        Ok(data) => {
            TypedInvocationResponse::success(data, Some(format!("Completed {}.", request.command)))
        }
        Err(error) => TypedInvocationResponse::error(CommandError::from_diagnostic(
            error
                .diagnostic()
                .with_domain("http")
                .with_operation(request.command.clone()),
            retryable_http_error(error.code()),
        )),
    }
}

fn typed_request(
    request: &TypedInvocationRequest,
    command_name: &'static str,
    method: Option<&str>,
) -> Result<Value, AppError> {
    let method = method
        .map(str::to_owned)
        .or_else(|| optional_string(&request.arguments, "method"))
        .ok_or_else(|| AppError::invalid_argument("missing HTTP method"))?;
    let args = RequestArgs {
        method,
        url: required_string(&request.arguments, "url")?,
        request: typed_request_options(request)?,
        expect: typed_expectations(&request.arguments),
    };
    let output = domain::run_request_command(args, command_name)?;
    if !output.ok {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} expectation(s) failed: {}",
                output.assertions.failed,
                output.assertions.failures.join("; ")
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

fn typed_replay(request: &TypedInvocationRequest) -> Result<Value, AppError> {
    let args = ReplayArgs {
        curl: required_string(&request.arguments, "curl")?,
        request: typed_request_options(request)?,
        expect: typed_expectations(&request.arguments),
    };
    let output = domain::run_replay(args, "replay")?;
    if !output.ok {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} expectation(s) failed: {}",
                output.assertions.failed,
                output.assertions.failures.join("; ")
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

fn typed_assert(
    request: &TypedInvocationRequest,
    command_name: &'static str,
) -> Result<Value, AppError> {
    let spec_path = resolve_context_path(
        &request.context.cwd,
        &required_string(&request.arguments, "spec_path")?,
    );
    let args = AssertArgs {
        spec_path,
        vars: string_array(&request.arguments, "vars"),
        fail_fast: bool_or(&request.arguments, "fail_fast", false),
        report: None,
    };
    let (output, _) = domain::run_assert(args, crate::output::OutputMode::Json, command_name)?;
    if output.summary.failed > 0 {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!(
                "{} of {} HTTP assertion case(s) failed",
                output.summary.failed, output.summary.total
            ),
        ));
    }
    Ok(serde_json::to_value(output)?)
}

fn typed_request_options(request: &TypedInvocationRequest) -> Result<RequestOptionsArgs, AppError> {
    let arguments = &request.arguments;
    let json = arguments
        .get("json")
        .map(serde_json::to_string)
        .transpose()?;
    Ok(RequestOptionsArgs {
        headers: string_array(arguments, "headers"),
        query: string_array(arguments, "query"),
        timeout_secs: Some(
            u64_or(arguments, "timeout_secs", domain::DEFAULT_TIMEOUT_SECS)
                .min(remaining_seconds(request)),
        ),
        max_response_bytes: arguments
            .get("max_response_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok()),
        bearer: optional_string(arguments, "bearer"),
        basic: optional_string(arguments, "basic"),
        json,
        json_file: optional_string(arguments, "json_file")
            .map(|path| resolve_context_path(&request.context.cwd, &path)),
        body: optional_string(arguments, "body"),
        body_file: optional_string(arguments, "body_file")
            .map(|path| resolve_context_path(&request.context.cwd, &path)),
    })
}

fn typed_expectations(arguments: &Value) -> RequestExpectArgs {
    RequestExpectArgs {
        expect_status: optional_string(arguments, "expect_status"),
        expect_headers: string_array(arguments, "expect_headers"),
        expect_body_contains: string_array(arguments, "expect_body_contains"),
        expect_json: string_array(arguments, "expect_json"),
    }
}

fn resolve_context_path(cwd: &str, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        PathBuf::from(cwd).join(path)
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

fn retryable_http_error(code: &str) -> bool {
    code.contains("HTTP_REQUEST") || code.contains("HTTP_RESPONSE") || code.contains("TIMEOUT")
}

fn required_string(arguments: &Value, name: &str) -> Result<String, AppError> {
    optional_string(arguments, name)
        .ok_or_else(|| AppError::invalid_argument(format!("missing {name}")))
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn string_array(arguments: &Value, name: &str) -> Vec<String> {
    arguments
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn bool_or(arguments: &Value, name: &str, default: bool) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn u64_or(arguments: &Value, name: &str, default: u64) -> u64 {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(default)
}

fn request_descriptor() -> CommandDescriptor {
    let mut properties = request_properties();
    properties.insert(
        "method".to_owned(),
        json!({"type": "string", "minLength": 1, "description": "HTTP method."}),
    );
    properties.insert("url".to_owned(), url_schema());
    CommandDescriptor::new(
        "http.request",
        "Send HTTP request",
        "Send an HTTP request with explicit method, payload, authentication, and expectations.",
        request_input_schema(properties, vec!["method", "url"]),
        request_output_schema("http.request"),
        http_write_effects(
            "Sends an arbitrary HTTP method and optional credentials or payload to an arbitrary URL; the remote service may mutate state.",
        ),
    )
}

fn shortcut_descriptor(command: &str, method: &str, read_only: bool) -> CommandDescriptor {
    let mut properties = request_properties();
    properties.insert("url".to_owned(), url_schema());
    CommandDescriptor::new(
        format!("http.{command}"),
        format!("Send HTTP {method}"),
        format!("Send an HTTP {method} request with payload, authentication, and expectations."),
        request_input_schema(properties, vec!["url"]),
        request_output_schema(&format!("http.{command}")),
        if read_only {
            http_read_effects(
                "Sends an HTTP GET request and optional credentials to an arbitrary URL; servers can still implement side effects for GET.",
            )
        } else {
            http_write_effects(&format!(
                "Sends an HTTP {method} request and optional credentials or payload to an arbitrary URL; the remote service may mutate state."
            ))
        },
    )
}

fn replay_descriptor() -> CommandDescriptor {
    let mut properties = request_properties();
    properties.insert(
        "curl".to_owned(),
        json!({"type": "string", "minLength": 1, "description": "Supported curl command form."}),
    );
    CommandDescriptor::new(
        "http.replay",
        "Replay curl request",
        "Parse and replay a supported curl command with optional expectation overrides.",
        request_input_schema(properties, vec!["curl"]),
        request_output_schema("http.replay"),
        http_write_effects(
            "Replays an arbitrary HTTP request encoded in curl syntax and may send embedded credentials or mutate a remote service.",
        ),
    )
}

fn assert_descriptor(command: &str) -> CommandDescriptor {
    CommandDescriptor::new(
        format!("http.{command}"),
        if command == "assert" {
            "Run HTTP assertions"
        } else {
            "Run HTTP assertion alias"
        },
        "Execute all HTTP cases in a YAML or JSON assertion spec.",
        json!({
            "type": "object",
            "properties": {
                "spec_path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "YAML or JSON spec path resolved against the execution cwd."
                },
                "vars": {
                    "type": "array",
                    "items": {"type": "string", "pattern": "^[^=]+=.*$"},
                    "description": "Template variables encoded as KEY=VALUE."
                },
                "fail_fast": {
                    "type": "boolean",
                    "default": false,
                    "description": "Stop after the first failed case."
                }
            },
            "required": ["spec_path"],
            "additionalProperties": false
        }),
        assert_output_schema(),
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

fn request_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert(
        "headers".to_owned(),
        string_list_schema("HTTP headers as K: V."),
    );
    properties.insert(
        "query".to_owned(),
        string_list_schema("Query parameters as KEY=VALUE."),
    );
    properties.insert(
        "timeout_secs".to_owned(),
        positive_integer_with_default(domain::DEFAULT_TIMEOUT_SECS, "HTTP timeout."),
    );
    properties.insert(
        "max_response_bytes".to_owned(),
        positive_integer_with_default(
            domain::DEFAULT_MAX_RESPONSE_BYTES as u64,
            "Maximum response body bytes.",
        ),
    );
    properties.insert(
        "bearer".to_owned(),
        json!({"type": "string", "description": "Bearer token sent to the target URL."}),
    );
    properties.insert(
        "basic".to_owned(),
        json!({"type": "string", "description": "Basic credentials as USER:PASS."}),
    );
    properties.insert(
        "json".to_owned(),
        json!({"description": "JSON request payload."}),
    );
    properties.insert(
        "json_file".to_owned(),
        file_path_schema("JSON payload file."),
    );
    properties.insert(
        "body".to_owned(),
        json!({"type": "string", "description": "Raw text request payload."}),
    );
    properties.insert(
        "body_file".to_owned(),
        file_path_schema("Raw text payload file."),
    );
    properties.insert(
        "expect_status".to_owned(),
        json!({"type": "string", "description": "Expected status code, class, or range."}),
    );
    properties.insert(
        "expect_headers".to_owned(),
        string_list_schema("Expected headers as K: V."),
    );
    properties.insert(
        "expect_body_contains".to_owned(),
        string_list_schema("Required body substrings."),
    );
    properties.insert(
        "expect_json".to_owned(),
        string_list_schema("JSON checks as PATH:OP[:VALUE]."),
    );
    properties
}

fn request_input_schema(properties: Map<String, Value>, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "description": "bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn url_schema() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "Absolute HTTP(S) URL. Network access is unrestricted by AIHelper."
    })
}

fn file_path_schema(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}

fn string_list_schema(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": description})
}

fn positive_integer_with_default(default: u64, description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": default,
        "description": description
    })
}

fn request_output_schema(command: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": command},
            "method": {"type": "string"},
            "url": {"type": "string"},
            "status": {"type": "integer", "minimum": 100, "maximum": 599},
            "ok": {"type": "boolean"},
            "duration_ms": {"type": "integer", "minimum": 0},
            "truncated": {"type": "boolean"},
            "body_truncated": {"type": "boolean"},
            "headers": {
                "type": "object",
                "additionalProperties": {"type": "string"}
            },
            "body": {"type": "string"},
            "assertions": {
                "type": "object",
                "properties": {
                    "total": {"type": "integer", "minimum": 0},
                    "passed": {"type": "integer", "minimum": 0},
                    "failed": {"type": "integer", "minimum": 0},
                    "failures": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["total", "passed", "failed", "failures"],
                "additionalProperties": false
            }
        },
        "required": [
            "command",
            "method",
            "url",
            "status",
            "ok",
            "duration_ms",
            "truncated",
            "body_truncated",
            "headers",
            "body",
            "assertions"
        ],
        "additionalProperties": false
    })
}

fn assert_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {"type": "string", "const": "http.assert"},
            "spec_path": {"type": "string"},
            "fail_fast": {"type": "boolean"},
            "summary": {
                "type": "object",
                "properties": {
                    "total": {"type": "integer", "minimum": 0},
                    "passed": {"type": "integer", "minimum": 0},
                    "failed": {"type": "integer", "minimum": 0},
                    "duration_ms": {"type": "integer", "minimum": 0}
                },
                "required": ["total", "passed", "failed", "duration_ms"],
                "additionalProperties": false
            },
            "cases": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "passed": {"type": "boolean"},
                        "status": {"type": ["integer", "null"], "minimum": 100, "maximum": 599},
                        "duration_ms": {"type": "integer", "minimum": 0},
                        "failures": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["name", "passed", "status", "duration_ms", "failures"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["command", "spec_path", "fail_fast", "summary", "cases"],
        "additionalProperties": false
    })
}

fn http_read_effects(impact: &str) -> CommandEffects {
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

fn http_write_effects(impact: &str) -> CommandEffects {
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

fn execute_shortcut(
    command_name: &'static str,
    method: &str,
    args: MethodShortcutArgs,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    execute_request(
        domain::run_request_shortcut(command_name, method, args),
        options,
    )
}

fn execute_request(
    request: Result<domain::HttpRequestOutput, AppError>,
    options: &GlobalOptions,
) -> Result<(), AppError> {
    let payload = request?;
    let failed = !payload.ok;
    adapters::output::emit_request(payload, options)?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            "request expectations failed",
        ));
    }
    Ok(())
}

fn execute_assert(
    args: AssertArgs,
    options: &GlobalOptions,
    command_name: &'static str,
) -> Result<(), AppError> {
    let (output, report_format) = domain::run_assert(args, options.output, command_name)?;
    let failed = output.summary.failed > 0;
    adapters::output::emit_assert(&output, report_format, options)?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            format!("{} of {} case(s) failed", failed, output.summary.total),
        ));
    }
    Ok(())
}
