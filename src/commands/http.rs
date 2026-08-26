use std::{
    collections::BTreeMap,
    fmt,
    path::PathBuf,
    time::{Duration, Instant},
};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError,
    InvocationResponse, ResolvedSecret, Reversibility, RiskLevel, SecretSlot,
    TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use clap::{Args, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::{cli::GlobalOptions, error::AppError, output::Emitter};

mod io;
mod output;

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

/// The wire contract for every request-shaped HTTP command, kept separate from
/// [`RequestOptionsArgs`] because the two genuinely differ: a caller sends a JSON
/// value, the CLI type holds it already serialized; a caller names a credential
/// slot, the CLI type holds the credential the host resolved; and the deadline is
/// supplied by the execution context, not by anyone. [`Self::split`] converts one
/// into the other with what only the execution context knows.
///
/// `credentials` is not part of the published input schema - the runtime augments
/// the MCP-facing schema with it from the descriptor's secret slots - but it does
/// arrive in the arguments, so the field exists and is hidden from the schema.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestOptionsWireArgs {
    /// HTTP headers as K: V.
    headers: Option<Vec<String>>,
    /// Query parameters as KEY=VALUE.
    query: Option<Vec<String>>,
    /// HTTP timeout.
    #[serde(default = "default_timeout_secs")]
    #[schemars(range(min = 1))]
    timeout_secs: u64,
    /// Maximum response body bytes.
    #[serde(default = "default_max_response_bytes")]
    #[schemars(range(min = 1))]
    max_response_bytes: usize,
    /// Additional attempts for transport failures, timeouts, response read failures, and HTTP 5xx.
    #[serde(default)]
    retry: u64,
    /// Fixed delay in milliseconds between retry attempts.
    #[serde(default)]
    retry_delay_ms: u64,
    /// Bearer token sent to the target URL.
    bearer: Option<String>,
    /// Basic credentials as USER:PASS.
    basic: Option<String>,
    /// JSON request payload.
    json: Option<Value>,
    /// JSON payload file.
    #[schemars(length(min = 1))]
    json_file: Option<String>,
    /// Raw text request payload.
    body: Option<String>,
    /// Raw text payload file.
    #[schemars(length(min = 1))]
    body_file: Option<String>,
    /// Expected status code, class, or range.
    expect_status: Option<String>,
    /// Expected headers as K: V.
    expect_headers: Option<Vec<String>>,
    /// Required body substrings.
    expect_body_contains: Option<Vec<String>>,
    /// JSON checks as PATH:OP[:VALUE].
    expect_json: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(skip)]
    credentials: Option<Value>,
}

fn default_timeout_secs() -> u64 {
    domain::DEFAULT_TIMEOUT_SECS
}

fn default_max_response_bytes() -> usize {
    domain::DEFAULT_MAX_RESPONSE_BYTES
}

impl RequestOptionsWireArgs {
    fn split(
        self,
        request: &TypedInvocationRequest,
    ) -> Result<(RequestOptionsArgs, RequestExpectArgs), AppError> {
        let cwd = &request.context.cwd;
        let options = RequestOptionsArgs {
            headers: self.headers.unwrap_or_default(),
            query: self.query.unwrap_or_default(),
            timeout_secs: Some(self.timeout_secs),
            max_response_bytes: Some(self.max_response_bytes),
            retry: self.retry,
            retry_delay_ms: self.retry_delay_ms,
            bearer: self.bearer,
            basic: self.basic,
            resolved_basic: resolved_basic_credential(
                self.credentials.as_ref(),
                &request.resolved_secrets,
            )?,
            json: self.json.as_ref().map(serde_json::to_string).transpose()?,
            json_file: self.json_file.map(|path| resolve_context_path(cwd, &path)),
            body: self.body,
            body_file: self.body_file.map(|path| resolve_context_path(cwd, &path)),
            deadline: request_deadline(request),
        };
        let expect = RequestExpectArgs {
            expect_status: self.expect_status,
            expect_headers: self.expect_headers.unwrap_or_default(),
            expect_body_contains: self.expect_body_contains.unwrap_or_default(),
            expect_json: self.expect_json.unwrap_or_default(),
        };
        Ok((options, expect))
    }
}

// The three request-shaped commands differ only in how the target is named, so
// the options above are flattened in. `deny_unknown_fields` cannot be combined
// with `serde(flatten)`, so each of them states the `additionalProperties: false`
// the catalog publishes; the runtime validates arguments against that schema
// before dispatch, which is what rejects an unknown property.
/// bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub struct RequestWireArgs {
    /// HTTP method.
    #[schemars(length(min = 1))]
    method: String,
    /// Absolute HTTP(S) URL. Network access is unrestricted by AIHelper.
    #[schemars(length(min = 1))]
    url: String,
    #[serde(flatten)]
    options: RequestOptionsWireArgs,
}

/// bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub struct MethodShortcutWireArgs {
    /// Absolute HTTP(S) URL. Network access is unrestricted by AIHelper.
    #[schemars(length(min = 1))]
    url: String,
    #[serde(flatten)]
    options: RequestOptionsWireArgs,
}

/// bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub struct ReplayWireArgs {
    /// Supported curl command form.
    #[schemars(length(min = 1))]
    curl: String,
    #[serde(flatten)]
    options: RequestOptionsWireArgs,
}

// The wire contract for `http.assert` and `http.run`: the report format is an
// output-shaping CLI flag and the deadline comes from the execution context, so
// neither appears here. Deliberately not a doc comment: these two commands
// publish no top-level schema description, and a doc comment would become one.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssertWireArgs {
    /// YAML or JSON spec path resolved against the execution cwd.
    #[schemars(length(min = 1))]
    spec_path: String,
    /// Template variables encoded as KEY=VALUE.
    #[schemars(inner(pattern(r"^[^=]+=.*$")))]
    vars: Option<Vec<String>>,
    /// Additional attempts per case for transport failures, timeouts, response read failures, and HTTP 5xx.
    #[serde(default)]
    retry: u64,
    /// Fixed delay in milliseconds between retry attempts.
    #[serde(default)]
    retry_delay_ms: u64,
    /// Stop after the first failed case.
    #[serde(default)]
    fail_fast: bool,
}

impl AssertWireArgs {
    fn into_args(self, request: &TypedInvocationRequest) -> AssertArgs {
        AssertArgs {
            spec_path: resolve_context_path(&request.context.cwd, &self.spec_path),
            vars: self.vars.unwrap_or_default(),
            retry: self.retry,
            retry_delay_ms: self.retry_delay_ms,
            fail_fast: self.fail_fast,
            report: None,
            deadline: request_deadline(request),
        }
    }
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
    let args = match method {
        Some(method) => {
            let wire: MethodShortcutWireArgs = decode(request)?;
            let (options, expect) = wire.options.split(request)?;
            RequestArgs {
                method: method.to_owned(),
                url: wire.url,
                request: options,
                expect,
            }
        }
        None => {
            let wire: RequestWireArgs = decode(request)?;
            let (options, expect) = wire.options.split(request)?;
            RequestArgs {
                method: wire.method,
                url: wire.url,
                request: options,
                expect,
            }
        }
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
    let wire: ReplayWireArgs = decode(request)?;
    let (options, expect) = wire.options.split(request)?;
    let args = ReplayArgs {
        curl: wire.curl,
        request: options,
        expect,
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
    let wire: AssertWireArgs = decode(request)?;
    let args = wire.into_args(request);
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

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the wire type disagree.
fn decode<T: serde::de::DeserializeOwned>(request: &TypedInvocationRequest) -> Result<T, AppError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        AppError::invalid_argument(format!(
            "invalid arguments for {}: {error}",
            request.command
        ))
    })
}

fn resolved_basic_credential(
    credentials: Option<&Value>,
    resolved_secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<Option<BasicCredential>, AppError> {
    let credentials = credentials.and_then(Value::as_object);
    if let Some(slot) = credentials.and_then(|items| items.keys().find(|key| *key != "basic")) {
        return Err(AppError::invalid_argument(format!(
            "unsupported HTTP credential slot '{slot}'"
        )));
    }
    let public_id = credentials
        .and_then(|items| items.get("basic"))
        .and_then(Value::as_str);
    let resolved = basic_from_resolved_secrets(resolved_secrets)?;
    match (public_id, &resolved) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(AppError::external(
                "SECRET_REQUIRED",
                "HTTP Basic credential was not resolved",
            ));
        }
    }
    if let Some((public_id, secret)) = public_id.zip(resolved_secrets.get("basic"))
        && secret.id != public_id
    {
        return Err(AppError::external(
            "SECRET_KIND_MISMATCH",
            "resolved HTTP Basic credential does not match the selected credential",
        ));
    }
    Ok(resolved)
}

/// Binds `--credential basic=ID` on the direct CLI path. The host resolved the
/// mapping, so there is no public credential map to cross-check here.
pub(crate) fn bind_resolved_credentials(
    args: &mut HttpArgs,
    secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<(), InvocationResponse> {
    if secrets.is_empty() {
        return Ok(());
    }
    let credential = basic_from_resolved_secrets(secrets).map_err(|error| {
        InvocationResponse::error_diagnostic(error.diagnostic().with_domain("http"))
    })?;
    let request = match &mut args.command {
        HttpCommand::Request(args) => &mut args.request,
        HttpCommand::Get(args) | HttpCommand::Post(args) => &mut args.request,
        HttpCommand::Replay(args) => &mut args.request,
        _ => {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "--credential is supported for http request, get, post, and replay",
            )
            .with_error_domain("http"));
        }
    };
    request.resolved_basic = credential;
    Ok(())
}

/// Shared by the typed and direct CLI paths: validates the slot and shape of the
/// credential the host resolved.
fn basic_from_resolved_secrets(
    secrets: &BTreeMap<String, ResolvedSecret>,
) -> Result<Option<BasicCredential>, AppError> {
    if let Some(slot) = secrets.keys().find(|slot| slot.as_str() != "basic") {
        return Err(AppError::invalid_argument(format!(
            "unsupported resolved HTTP credential slot '{slot}'"
        )));
    }
    let Some(resolved) = secrets.get("basic") else {
        return Ok(None);
    };
    if resolved.kind != "http-basic" {
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

fn retryable_http_error(code: &str) -> bool {
    code.contains("HTTP_REQUEST") || code.contains("HTTP_RESPONSE") || code.contains("TIMEOUT")
}

fn request_descriptor() -> CommandDescriptor {
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

fn shortcut_descriptor(command: &str, method: &str, read_only: bool) -> CommandDescriptor {
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

fn replay_descriptor() -> CommandDescriptor {
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
    output::emit_request(payload, options.limit, &mut Emitter::stdio(options))?;
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
    output::emit_assert(&output, report_format, &mut Emitter::stdio(options))?;
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
    use serde_json::json;

    use super::*;

    fn typed_request_options(
        request: &TypedInvocationRequest,
    ) -> Result<RequestOptionsArgs, AppError> {
        let wire: MethodShortcutWireArgs = decode(request)?;
        wire.options.split(request).map(|(options, _)| options)
    }

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
