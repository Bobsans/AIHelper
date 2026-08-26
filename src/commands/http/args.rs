//! Every argument `ah http` accepts, in both shapes it accepts them: the clap
//! types a person types, and the wire types an MCP client sends.
//!
//! Declaration only. `http` has the widest flag surface of any domain, and
//! keeping it here is what lets the behaviour beside it stay readable.

use super::*;

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
    pub(super) username: String,
    pub(super) password: String,
}

impl BasicCredential {
    pub(crate) fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    pub(super) fn into_parts(self) -> (String, String) {
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
/// supplied by the execution context, not by anyone. `Self::split` converts one
/// into the other with what only the execution context knows.
///
/// `credentials` is not part of the published input schema - the runtime augments
/// the MCP-facing schema with it from the descriptor's secret slots - but it does
/// arrive in the arguments, so the field exists and is hidden from the schema.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestOptionsWireArgs {
    /// HTTP headers as K: V.
    pub(super) headers: Option<Vec<String>>,
    /// Query parameters as KEY=VALUE.
    pub(super) query: Option<Vec<String>>,
    /// HTTP timeout.
    #[serde(default = "default_timeout_secs")]
    #[schemars(range(min = 1))]
    pub(super) timeout_secs: u64,
    /// Maximum response body bytes.
    #[serde(default = "default_max_response_bytes")]
    #[schemars(range(min = 1))]
    pub(super) max_response_bytes: usize,
    /// Additional attempts for transport failures, timeouts, response read failures, and HTTP 5xx.
    #[serde(default)]
    pub(super) retry: u64,
    /// Fixed delay in milliseconds between retry attempts.
    #[serde(default)]
    pub(super) retry_delay_ms: u64,
    /// Bearer token sent to the target URL.
    pub(super) bearer: Option<String>,
    /// Basic credentials as USER:PASS.
    pub(super) basic: Option<String>,
    /// JSON request payload.
    pub(super) json: Option<Value>,
    /// JSON payload file.
    #[schemars(length(min = 1))]
    pub(super) json_file: Option<String>,
    /// Raw text request payload.
    pub(super) body: Option<String>,
    /// Raw text payload file.
    #[schemars(length(min = 1))]
    pub(super) body_file: Option<String>,
    /// Expected status code, class, or range.
    pub(super) expect_status: Option<String>,
    /// Expected headers as K: V.
    pub(super) expect_headers: Option<Vec<String>>,
    /// Required body substrings.
    pub(super) expect_body_contains: Option<Vec<String>>,
    /// JSON checks as PATH:OP[:VALUE].
    pub(super) expect_json: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(skip)]
    pub(super) credentials: Option<Value>,
}

pub(super) fn default_timeout_secs() -> u64 {
    domain::DEFAULT_TIMEOUT_SECS
}

pub(super) fn default_max_response_bytes() -> usize {
    domain::DEFAULT_MAX_RESPONSE_BYTES
}

impl RequestOptionsWireArgs {
    pub(super) fn split(
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
    pub(super) method: String,
    /// Absolute HTTP(S) URL. Network access is unrestricted by AIHelper.
    #[schemars(length(min = 1))]
    pub(super) url: String,
    #[serde(flatten)]
    pub(super) options: RequestOptionsWireArgs,
}

/// bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub struct MethodShortcutWireArgs {
    /// Absolute HTTP(S) URL. Network access is unrestricted by AIHelper.
    #[schemars(length(min = 1))]
    pub(super) url: String,
    #[serde(flatten)]
    pub(super) options: RequestOptionsWireArgs,
}

/// bearer and basic are mutually exclusive; at most one of json, json_file, body, or body_file may be supplied.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub struct ReplayWireArgs {
    /// Supported curl command form.
    #[schemars(length(min = 1))]
    pub(super) curl: String,
    #[serde(flatten)]
    pub(super) options: RequestOptionsWireArgs,
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
    pub(super) spec_path: String,
    /// Template variables encoded as KEY=VALUE.
    #[schemars(inner(pattern(r"^[^=]+=.*$")))]
    pub(super) vars: Option<Vec<String>>,
    /// Additional attempts per case for transport failures, timeouts, response read failures, and HTTP 5xx.
    #[serde(default)]
    pub(super) retry: u64,
    /// Fixed delay in milliseconds between retry attempts.
    #[serde(default)]
    pub(super) retry_delay_ms: u64,
    /// Stop after the first failed case.
    #[serde(default)]
    pub(super) fail_fast: bool,
}

impl AssertWireArgs {
    pub(super) fn into_args(self, request: &TypedInvocationRequest) -> AssertArgs {
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
