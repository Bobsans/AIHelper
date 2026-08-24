use std::{
    fmt,
    path::PathBuf,
    time::{Duration, Instant},
};

use ah_plugin_api::{
    CliTypedInvocation, CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects,
    CommandError, InvocationResponse, Reversibility, RiskLevel, SecretSlot, TypedInvocationRequest,
    TypedInvocationResponse,
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
    #[arg(long, default_value_t = 0, value_name = "COUNT")]
    pub retry: u64,
    #[arg(long, default_value_t = 0, value_name = "MILLISECONDS")]
    pub retry_delay_ms: u64,
    #[arg(long)]
    pub fail_fast: bool,
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub report: Option<AssertReportArg>,
    #[arg(skip)]
    pub deadline: Option<Instant>,
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
    #[arg(long, default_value_t = 0, value_name = "COUNT")]
    pub retry: u64,
    #[arg(long, default_value_t = 0, value_name = "MILLISECONDS")]
    pub retry_delay_ms: u64,
    #[arg(long, value_name = "TOKEN")]
    pub bearer: Option<String>,
    #[arg(long, value_name = "USER:PASS")]
    pub basic: Option<String>,
    #[arg(skip)]
    pub(crate) resolved_basic: Option<BasicCredential>,
    #[arg(long, value_name = "JSON")]
    pub json: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub json_file: Option<PathBuf>,
    #[arg(long, value_name = "TEXT")]
    pub body: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub body_file: Option<PathBuf>,
    #[arg(skip)]
    pub deadline: Option<Instant>,
}

#[derive(Clone)]
pub(crate) struct BasicCredential {
    username: String,
    password: String,
}

impl BasicCredential {
    pub(crate) fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    fn into_parts(self) -> (String, String) {
        (self.username, self.password)
    }
}

impl fmt::Debug for BasicCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
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

pub(crate) fn emit_typed_cli_data(data: Value, options: &GlobalOptions) -> Result<(), AppError> {
    let mut output = serde_json::from_value::<domain::HttpRequestOutput>(data)?;
    let failed = !output.ok;
    output.status_text = reqwest::StatusCode::from_u16(output.status)
        .ok()
        .and_then(|status| status.canonical_reason().map(str::to_owned))
        .unwrap_or_default();
    adapters::output::emit_request(output, options)?;
    if failed {
        return Err(AppError::external(
            "HTTP_ASSERTION_FAILED",
            "request expectations failed",
        ));
    }
    Ok(())
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

/// Converts only credential-capable HTTP CLI commands using the concrete clap model.
pub(crate) fn cli_to_typed(args: HttpArgs) -> Result<CliTypedInvocation, InvocationResponse> {
    let (command, mut arguments) = match args.command {
        HttpCommand::Request(args) => {
            let mut values = request_arguments(args.request, args.expect)?;
            values.insert("method".to_owned(), Value::String(args.method));
            values.insert("url".to_owned(), Value::String(args.url));
            ("http.request", values)
        }
        HttpCommand::Get(args) => shortcut_arguments("http.get", args)?,
        HttpCommand::Post(args) => shortcut_arguments("http.post", args)?,
        HttpCommand::Replay(args) => {
            let mut values = request_arguments(args.request, args.expect)?;
            values.insert("curl".to_owned(), Value::String(args.curl));
            ("http.replay", values)
        }
        _ => {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "--credential is supported for http request, get, post, and replay",
            )
            .with_error_domain("http"));
        }
    };
    Ok(CliTypedInvocation {
        command: command.to_owned(),
        arguments: Value::Object(std::mem::take(&mut arguments)),
    })
}

fn shortcut_arguments(
    command: &'static str,
    args: MethodShortcutArgs,
) -> Result<(&'static str, Map<String, Value>), InvocationResponse> {
    let mut values = request_arguments(args.request, args.expect)?;
    values.insert("url".to_owned(), Value::String(args.url));
    Ok((command, values))
}

fn request_arguments(
    request: RequestOptionsArgs,
    expect: RequestExpectArgs,
) -> Result<Map<String, Value>, InvocationResponse> {
    let mut values = Map::new();
    values.insert("headers".to_owned(), json!(request.headers));
    values.insert("query".to_owned(), json!(request.query));
    insert_option(&mut values, "timeout_secs", request.timeout_secs);
    insert_option(
        &mut values,
        "max_response_bytes",
        request.max_response_bytes,
    );
    values.insert("retry".to_owned(), json!(request.retry));
    values.insert("retry_delay_ms".to_owned(), json!(request.retry_delay_ms));
    insert_option(&mut values, "bearer", request.bearer);
    insert_option(&mut values, "basic", request.basic);
    if let Some(raw) = request.json {
        let parsed = serde_json::from_str::<Value>(&raw).map_err(|error| {
            InvocationResponse::error("INVALID_ARGUMENT", format!("invalid --json value: {error}"))
                .with_error_domain("http")
        })?;
        values.insert("json".to_owned(), parsed);
    }
    insert_path(&mut values, "json_file", request.json_file);
    insert_option(&mut values, "body", request.body);
    insert_path(&mut values, "body_file", request.body_file);
    insert_option(&mut values, "expect_status", expect.expect_status);
    values.insert("expect_headers".to_owned(), json!(expect.expect_headers));
    values.insert(
        "expect_body_contains".to_owned(),
        json!(expect.expect_body_contains),
    );
    values.insert("expect_json".to_owned(), json!(expect.expect_json));
    Ok(values)
}

fn insert_option<T: serde::Serialize>(
    values: &mut Map<String, Value>,
    key: &str,
    value: Option<T>,
) {
    if let Some(value) = value {
        values.insert(key.to_owned(), json!(value));
    }
}

fn insert_path(values: &mut Map<String, Value>, key: &str, value: Option<PathBuf>) {
    if let Some(value) = value {
        values.insert(
            key.to_owned(),
            Value::String(value.to_string_lossy().into_owned()),
        );
    }
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
    if !output.ok && !is_direct_cli_request(request) {
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
    if !output.ok && !is_direct_cli_request(request) {
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
        retry: u64_or(&request.arguments, "retry", 0),
        retry_delay_ms: u64_or(&request.arguments, "retry_delay_ms", 0),
        fail_fast: bool_or(&request.arguments, "fail_fast", false),
        report: None,
        deadline: request_deadline(request),
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
        timeout_secs: Some(u64_or(
            arguments,
            "timeout_secs",
            domain::DEFAULT_TIMEOUT_SECS,
        )),
        max_response_bytes: arguments
            .get("max_response_bytes")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok()),
        retry: u64_or(arguments, "retry", 0),
        retry_delay_ms: u64_or(arguments, "retry_delay_ms", 0),
        bearer: optional_string(arguments, "bearer"),
        basic: optional_string(arguments, "basic"),
        resolved_basic: resolved_basic_credential(request)?,
        json,
        json_file: optional_string(arguments, "json_file")
            .map(|path| resolve_context_path(&request.context.cwd, &path)),
        body: optional_string(arguments, "body"),
        body_file: optional_string(arguments, "body_file")
            .map(|path| resolve_context_path(&request.context.cwd, &path)),
        deadline: request_deadline(request),
    })
}

fn resolved_basic_credential(
    request: &TypedInvocationRequest,
) -> Result<Option<BasicCredential>, AppError> {
    let credentials = request
        .arguments
        .get("credentials")
        .and_then(Value::as_object);
    if let Some(slot) = credentials.and_then(|items| items.keys().find(|key| *key != "basic")) {
        return Err(AppError::invalid_argument(format!(
            "unsupported HTTP credential slot '{slot}'"
        )));
    }
    if let Some(slot) = request
        .resolved_secrets
        .keys()
        .find(|slot| slot.as_str() != "basic")
    {
        return Err(AppError::invalid_argument(format!(
            "unsupported resolved HTTP credential slot '{slot}'"
        )));
    }

    let public_id = credentials
        .and_then(|items| items.get("basic"))
        .and_then(Value::as_str);
    let resolved = request.resolved_secrets.get("basic");
    let Some((public_id, resolved)) = public_id.zip(resolved) else {
        return match (public_id, resolved) {
            (None, None) => Ok(None),
            _ => Err(AppError::external(
                "SECRET_REQUIRED",
                "HTTP Basic credential was not resolved",
            )),
        };
    };
    if resolved.id != public_id || resolved.kind != "http-basic" {
        return Err(AppError::external(
            "SECRET_KIND_MISMATCH",
            "resolved HTTP Basic credential does not match the selected credential",
        ));
    }
    let username = resolved.values.get("username").ok_or_else(|| {
        AppError::external(
            "SECRET_REQUIRED",
            "resolved HTTP Basic credential has no username",
        )
    })?;
    let password = resolved.values.get("password").ok_or_else(|| {
        AppError::external(
            "SECRET_REQUIRED",
            "resolved HTTP Basic credential has no password",
        )
    })?;
    if username.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "resolved HTTP Basic username must not be empty",
        ));
    }
    Ok(Some(BasicCredential::new(username, password)))
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

fn request_deadline(request: &TypedInvocationRequest) -> Option<Instant> {
    Instant::now().checked_add(Duration::from_millis(request.context.remaining_timeout_ms))
}

fn is_direct_cli_request(request: &TypedInvocationRequest) -> bool {
    request.context.remaining_timeout_ms == u64::MAX
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
    .with_secret_slot(http_basic_slot())
}

fn shortcut_descriptor(command: &str, method: &str, read_only: bool) -> CommandDescriptor {
    let mut properties = request_properties();
    properties.insert("url".to_owned(), url_schema());
    let descriptor = CommandDescriptor::new(
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
    );
    if matches!(command, "get" | "post") {
        descriptor.with_secret_slot(http_basic_slot())
    } else {
        descriptor
    }
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
    .with_secret_slot(http_basic_slot())
}

fn http_basic_slot() -> SecretSlot {
    SecretSlot::optional("basic", ["http-basic"], "HTTP Basic credential.")
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
                "retry": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Additional attempts per case for transport failures, timeouts, response read failures, and HTTP 5xx."
                },
                "retry_delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "default": 0,
                    "description": "Fixed delay in milliseconds between retry attempts."
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
        "retry".to_owned(),
        nonnegative_integer_with_default(
            0,
            "Additional attempts for transport failures, timeouts, response read failures, and HTTP 5xx.",
        ),
    );
    properties.insert(
        "retry_delay_ms".to_owned(),
        nonnegative_integer_with_default(0, "Fixed delay in milliseconds between retry attempts."),
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

fn nonnegative_integer_with_default(default: u64, description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
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
            format!(
                "{} of {} HTTP assertion case(s) failed",
                output.summary.failed, output.summary.total
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ah_plugin_api::{ExecutionContextWire, ResolvedSecret};

    use super::*;

    #[test]
    fn typed_http_schemas_expose_retry_options() {
        let catalog = command_catalog();
        for command_id in [
            "http.request",
            "http.get",
            "http.replay",
            "http.assert",
            "http.run",
        ] {
            let command = catalog
                .commands
                .iter()
                .find(|command| command.id == command_id)
                .expect("command should exist");
            assert_eq!(command.input_schema["properties"]["retry"]["minimum"], 0);
            assert_eq!(
                command.input_schema["properties"]["retry_delay_ms"]["minimum"],
                0
            );
        }
    }

    #[test]
    fn only_request_get_post_and_replay_declare_http_basic_slot() {
        let catalog = command_catalog();

        for command in &catalog.commands {
            let expected = matches!(
                command.id.as_str(),
                "http.request" | "http.get" | "http.post" | "http.replay"
            );
            assert_eq!(
                command.secret_slots.len(),
                usize::from(expected),
                "{}",
                command.id
            );
            if expected {
                assert_eq!(command.secret_slots[0].name, "basic");
                assert_eq!(command.secret_slots[0].accepted_kinds, ["http-basic"]);
                assert!(!command.secret_slots[0].required);
            }
        }
    }

    #[test]
    fn typed_request_options_bind_matching_resolved_basic_auth() {
        let request = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-secret", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    ("password".to_owned(), "http-private-sentinel".to_owned()),
                ]),
            },
        )]));

        let options = typed_request_options(&request).expect("credential should bind");
        let basic = options.resolved_basic.expect("resolved basic auth");
        assert_eq!(basic.username, "vault-user");
        assert_eq!(basic.password, "http-private-sentinel");
    }

    #[test]
    fn typed_http_rejects_unresolved_and_wrong_slots_without_leaking_values() {
        let unresolved = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-unresolved", ".", None, 1_000),
        );
        assert_eq!(
            typed_request_options(&unresolved)
                .expect_err("public id requires private resolution")
                .code(),
            "SECRET_REQUIRED"
        );

        let wrong_slot = TypedInvocationRequest::new(
            "http.get",
            json!({"url": "https://example.test", "credentials": {"basic": "api"}}),
            ExecutionContextWire::new("http-wrong-slot", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "other".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "leak-user".to_owned()),
                    ("password".to_owned(), "http-leak-sentinel".to_owned()),
                ]),
            },
        )]));
        let response = invoke_typed(&wrong_slot);
        let serialized = serde_json::to_string(&response).expect("response serializes");
        assert!(!response.success);
        assert!(!serialized.contains("leak-user"));
        assert!(!serialized.contains("http-leak-sentinel"));
    }

    #[test]
    fn typed_http_rejects_authorization_header_with_resolved_basic() {
        let request = TypedInvocationRequest::new(
            "http.get",
            json!({
                "url": "https://example.test",
                "headers": ["aUtHoRiZaTiOn: Bearer raw-header-sentinel"],
                "credentials": {"basic": "api"}
            }),
            ExecutionContextWire::new("http-auth-header", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    (
                        "password".to_owned(),
                        "http-header-password-sentinel".to_owned(),
                    ),
                ]),
            },
        )]));

        let response = invoke_typed(&request);
        let error = response.error.expect("conflict should fail");
        let serialized = serde_json::to_string(&error).expect("error serializes");
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("raw-header-sentinel"));
        assert!(!serialized.contains("http-header-password-sentinel"));
    }

    #[test]
    fn typed_replay_rejects_curl_authorization_header_with_resolved_basic() {
        let request = TypedInvocationRequest::new(
            "http.replay",
            json!({
                "curl": "curl https://example.test -H 'AUTHORIZATION: Bearer replay-header-sentinel'",
                "credentials": {"basic": "api"}
            }),
            ExecutionContextWire::new("http-replay-auth-header", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "basic".to_owned(),
            ResolvedSecret {
                id: "api".to_owned(),
                kind: "http-basic".to_owned(),
                values: BTreeMap::from([
                    ("username".to_owned(), "vault-user".to_owned()),
                    ("password".to_owned(), "http-replay-password-sentinel".to_owned()),
                ]),
            },
        )]));

        let response = invoke_typed(&request);
        let error = response.error.expect("replay conflict should fail");
        let serialized = serde_json::to_string(&error).expect("error serializes");
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("replay-header-sentinel"));
        assert!(!serialized.contains("http-replay-password-sentinel"));
    }
}
