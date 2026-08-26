use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AppError;

use super::{
    BasicCredential, MethodShortcutArgs, ReplayArgs, RequestArgs, RequestOptionsArgs, adapters,
};

mod assert;
mod curl;
#[cfg(test)]
mod hostile_input;
mod jsonpath;
mod spec;

use assert::{evaluate_assertions, parse_request_expectations};
use curl::parse_curl_replay;
pub(crate) use spec::{render_assert_junit, run_assert};

pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(crate) const DEFAULT_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct RequestConfig {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) query: Vec<(String, String)>,
    pub(crate) timeout: Duration,
    pub(crate) max_response_bytes: usize,
    pub(crate) auth: AuthConfig,
    pub(crate) body: Option<RequestBody>,
}

#[derive(Clone)]
pub(crate) enum AuthConfig {
    None,
    Bearer(String),
    Basic { username: String, password: String },
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("None"),
            Self::Bearer(_) => formatter.write_str("Bearer([REDACTED])"),
            Self::Basic { .. } => formatter.write_str("Basic([REDACTED])"),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum RequestBody {
    Json(Value),
    Text(String),
}

#[derive(Debug)]
pub(crate) struct ResponseSnapshot {
    pub(crate) status_code: u16,
    pub(crate) status_text: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
    pub(crate) body_json: Option<Value>,
    pub(crate) body_truncated: bool,
}

#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct AssertionSummary {
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) failures: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct HttpRequestOutput {
    pub(crate) command: String,
    pub(crate) method: String,
    pub(crate) url: String,
    #[schemars(range(min = 100, max = 599))]
    pub(crate) status: u16,
    #[serde(skip, default)]
    pub(crate) status_text: String,
    pub(crate) ok: bool,
    pub(crate) duration_ms: u64,
    pub(crate) truncated: bool,
    pub(crate) body_truncated: bool,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
    pub(crate) assertions: AssertionSummary,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HttpAssertOutput {
    pub(crate) command: &'static str,
    pub(crate) spec_path: String,
    pub(crate) fail_fast: bool,
    pub(crate) summary: HttpAssertSummary,
    pub(crate) cases: Vec<HttpAssertCaseOutput>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HttpAssertSummary {
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) duration_ms: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HttpAssertCaseOutput {
    pub(crate) name: String,
    pub(crate) passed: bool,
    #[schemars(range(min = 100, max = 599))]
    pub(crate) status: Option<u16>,
    pub(crate) duration_ms: u64,
    pub(crate) failures: Vec<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum AssertReportFormat {
    Text,
    Json,
    Junit,
}

pub(crate) fn run_request_shortcut(
    command_name: &'static str,
    method: &str,
    args: MethodShortcutArgs,
) -> Result<HttpRequestOutput, AppError> {
    let request_args = RequestArgs {
        method: method.to_owned(),
        url: args.url,
        request: args.request,
        expect: args.expect,
    };
    run_request_command(request_args, command_name)
}

pub(crate) fn run_request_command(
    args: RequestArgs,
    command_name: &'static str,
) -> Result<HttpRequestOutput, AppError> {
    let expectations = parse_request_expectations(&args.expect)?;
    let request = build_request_config(&args.method, &args.url, &args.request)?;
    let started = std::time::Instant::now();
    let response = send_with_retry(
        &request,
        args.request.retry,
        args.request.retry_delay_ms,
        args.request.deadline,
    )?;
    let duration_ms = duration_millis(started.elapsed());
    let assertions = evaluate_assertions(&response, &expectations);

    Ok(HttpRequestOutput {
        command: format!("http.{command_name}"),
        method: request.method,
        url: request.url,
        status: response.status_code,
        status_text: response.status_text,
        ok: assertions.failed == 0,
        duration_ms,
        truncated: response.body_truncated,
        body_truncated: response.body_truncated,
        headers: response.headers,
        body: response.body,
        assertions,
    })
}

fn send_with_retry(
    request: &RequestConfig,
    retries: u64,
    retry_delay_ms: u64,
    deadline: Option<Instant>,
) -> Result<ResponseSnapshot, AppError> {
    let mut retries_remaining = retries;

    loop {
        let mut attempt = request.clone();
        if let Some(remaining) = remaining_deadline(deadline)? {
            attempt.timeout = attempt.timeout.min(remaining);
        }

        match adapters::io::send_request(&attempt) {
            Ok(response) if response.status_code >= 500 && retries_remaining > 0 => {}
            Ok(response) => return Ok(response),
            Err(error) if is_retryable_request_error(&error) && retries_remaining > 0 => {}
            Err(error) => return Err(error),
        }

        retries_remaining -= 1;
        wait_for_retry(retry_delay_ms, deadline)?;
    }
}

fn remaining_deadline(deadline: Option<Instant>) -> Result<Option<Duration>, AppError> {
    let Some(deadline) = deadline else {
        return Ok(None);
    };
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .map(Some)
        .ok_or_else(http_deadline_exceeded)
}

fn wait_for_retry(retry_delay_ms: u64, deadline: Option<Instant>) -> Result<(), AppError> {
    let delay = Duration::from_millis(retry_delay_ms);
    if delay.is_zero() {
        return remaining_deadline(deadline).map(|_| ());
    }
    if let Some(remaining) = remaining_deadline(deadline)?
        && delay >= remaining
    {
        return Err(http_deadline_exceeded());
    }
    thread::sleep(delay);
    Ok(())
}

fn is_retryable_request_error(error: &AppError) -> bool {
    matches!(
        error.code(),
        "HTTP_REQUEST_FAILED" | "HTTP_RESPONSE_READ_FAILED"
    )
}

fn http_deadline_exceeded() -> AppError {
    AppError::external(
        "HTTP_TIMEOUT",
        "HTTP execution deadline expired before the next attempt",
    )
}

pub(crate) fn run_replay(
    args: ReplayArgs,
    command_name: &'static str,
) -> Result<HttpRequestOutput, AppError> {
    let parsed = parse_curl_replay(&args.curl)?;
    let mut request_args = args.request;

    request_args.headers.splice(
        0..0,
        parsed
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}")),
    );

    if request_args.timeout_secs.is_none() {
        request_args.timeout_secs = parsed.timeout_secs;
    }
    if request_args.bearer.is_none() && request_args.basic.is_none() {
        request_args.bearer = parsed.bearer;
        request_args.basic = parsed.basic;
    }
    if !has_explicit_payload(&request_args) {
        match parsed.body {
            Some(RequestBody::Json(value)) => {
                request_args.json = Some(serde_json::to_string(&value)?);
            }
            Some(RequestBody::Text(value)) => {
                request_args.body = Some(value);
            }
            None => {}
        }
    }

    let request = RequestArgs {
        method: parsed.method.unwrap_or_else(|| "GET".to_owned()),
        url: parsed.url,
        request: request_args,
        expect: args.expect,
    };

    run_request_command(request, command_name)
}

fn build_request_config(
    method: &str,
    url: &str,
    args: &RequestOptionsArgs,
) -> Result<RequestConfig, AppError> {
    let method_normalized = normalize_method(method)?;
    let parsed_url = url.trim();
    if parsed_url.is_empty() {
        return Err(AppError::invalid_argument("url must not be empty"));
    }

    let mut headers = Vec::new();
    for raw in &args.headers {
        let (name, value) = parse_header(raw, "--header")?;
        headers.push((name, value));
    }
    if args.resolved_basic.is_some()
        && headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    {
        return Err(AppError::invalid_argument(
            "Authorization header and vault Basic credentials are mutually exclusive",
        ));
    }

    let mut query = Vec::new();
    for raw in &args.query {
        let (name, value) = parse_key_value_pair(raw, "--query", '=')?;
        query.push((name, value));
    }

    let auth = parse_auth(
        args.bearer.as_deref(),
        args.basic.as_deref(),
        args.resolved_basic.clone(),
    )?;
    let body = parse_payload(args, None)?;

    let max_response_bytes = args
        .max_response_bytes
        .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);
    if max_response_bytes == 0 {
        return Err(AppError::invalid_argument(
            "--max-response-bytes must be >= 1",
        ));
    }

    Ok(RequestConfig {
        method: method_normalized,
        url: parsed_url.to_owned(),
        headers,
        query,
        timeout: Duration::from_secs(args.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1)),
        max_response_bytes,
        auth,
        body,
    })
}

pub(super) fn parse_payload(
    args: &RequestOptionsArgs,
    base_dir: Option<&Path>,
) -> Result<Option<RequestBody>, AppError> {
    let mut sources = 0usize;
    if args.json.is_some() {
        sources += 1;
    }
    if args.json_file.is_some() {
        sources += 1;
    }
    if args.body.is_some() {
        sources += 1;
    }
    if args.body_file.is_some() {
        sources += 1;
    }
    if sources > 1 {
        return Err(AppError::invalid_argument(
            "payload flags are mutually exclusive: use only one of --json, --json-file, --body, --body-file",
        ));
    }

    if let Some(raw_json) = args.json.as_ref() {
        let value: Value = serde_json::from_str(raw_json).map_err(|error| {
            AppError::invalid_argument(format!("--json is not valid JSON: {error}"))
        })?;
        return Ok(Some(RequestBody::Json(value)));
    }

    if let Some(json_path) = args.json_file.as_ref() {
        let resolved = resolve_file_path(base_dir, json_path);
        let raw = adapters::io::read_to_string(&resolved)?;
        let value: Value = serde_json::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse JSON file '{}': {error}",
                resolved.display()
            ))
        })?;
        return Ok(Some(RequestBody::Json(value)));
    }

    if let Some(body) = args.body.as_ref() {
        return Ok(Some(RequestBody::Text(body.clone())));
    }

    if let Some(body_path) = args.body_file.as_ref() {
        let resolved = resolve_file_path(base_dir, body_path);
        let raw = adapters::io::read_to_string(&resolved)?;
        return Ok(Some(RequestBody::Text(raw)));
    }

    Ok(None)
}

pub(super) fn parse_auth(
    bearer: Option<&str>,
    basic: Option<&str>,
    resolved_basic: Option<BasicCredential>,
) -> Result<AuthConfig, AppError> {
    match (bearer, basic, resolved_basic) {
        (Some(_), Some(_), _) => Err(AppError::invalid_argument(
            "--bearer and --basic are mutually exclusive",
        )),
        (_, Some(_), Some(_)) => Err(AppError::invalid_argument(
            "--basic and vault Basic credentials are mutually exclusive",
        )),
        (Some(_), None, Some(_)) => Err(AppError::invalid_argument(
            "--bearer and vault Basic credentials are mutually exclusive",
        )),
        (Some(token), None, None) => {
            if token.trim().is_empty() {
                return Err(AppError::invalid_argument("--bearer must not be empty"));
            }
            Ok(AuthConfig::Bearer(token.to_owned()))
        }
        (None, Some(raw), None) => {
            let (username, password) = parse_basic_auth(raw)?;
            Ok(AuthConfig::Basic { username, password })
        }
        (None, None, Some(resolved)) => {
            let (username, password) = resolved.into_parts();
            Ok(AuthConfig::Basic { username, password })
        }
        (None, None, None) => Ok(AuthConfig::None),
    }
}

pub(super) fn parse_basic_auth(raw: &str) -> Result<(String, String), AppError> {
    let (username, password) = raw
        .split_once(':')
        .ok_or_else(|| AppError::invalid_argument("--basic must use USER:PASS format"))?;
    if username.trim().is_empty() {
        return Err(AppError::invalid_argument(
            "--basic username must not be empty",
        ));
    }
    Ok((username.to_owned(), password.to_owned()))
}

pub(super) fn parse_header(raw: &str, flag_name: &str) -> Result<(String, String), AppError> {
    let (name, value) = raw
        .split_once(':')
        .ok_or_else(|| AppError::invalid_argument(format!("{flag_name} must use 'Name: Value'")))?;
    let name = name.trim();
    let value = value.trim();
    if name.is_empty() {
        return Err(AppError::invalid_argument(format!(
            "{flag_name} header name must not be empty"
        )));
    }
    Ok((name.to_owned(), value.to_owned()))
}

fn parse_key_value_pair(
    raw: &str,
    flag_name: &str,
    separator: char,
) -> Result<(String, String), AppError> {
    let (name, value) = raw.split_once(separator).ok_or_else(|| {
        AppError::invalid_argument(format!("{flag_name} must use KEY{separator}VALUE"))
    })?;
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid_argument(format!(
            "{flag_name} key must not be empty"
        )));
    }
    Ok((name.to_owned(), value.to_owned()))
}

pub(super) fn normalize_method(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::invalid_argument("method must not be empty"));
    }
    Ok(trimmed.to_ascii_uppercase())
}

fn has_explicit_payload(args: &RequestOptionsArgs) -> bool {
    args.json.is_some()
        || args.json_file.is_some()
        || args.body.is_some()
        || args.body_file.is_some()
}

pub(super) fn parse_timeout_secs(raw: &str, flag_name: &str) -> Result<u64, AppError> {
    let parsed = raw.parse::<f64>().map_err(|error| {
        AppError::invalid_argument(format!("{flag_name} value is invalid: {error}"))
    })?;
    if parsed <= 0.0 {
        return Err(AppError::invalid_argument(format!(
            "{flag_name} must be > 0"
        )));
    }
    Ok(parsed.ceil() as u64)
}

fn resolve_file_path(base_dir: Option<&Path>, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else if let Some(base) = base_dir {
        base.join(value)
    } else {
        value.to_path_buf()
    }
}

pub(super) fn resolve_spec_relative_path(base_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

fn duration_millis(value: Duration) -> u64 {
    value.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_respects_deadline_without_sleeping() {
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(1))
            .expect("deadline should fit");

        let error = wait_for_retry(1_000, Some(deadline)).expect_err("delay exceeds deadline");

        assert_eq!(error.code(), "HTTP_TIMEOUT");
    }

    #[test]
    fn legacy_and_resolved_basic_auth_cannot_coexist() {
        let resolved = BasicCredential::new("vault-user", "http-conflict-sentinel");

        let error = parse_auth(None, Some("legacy:password"), Some(resolved))
            .expect_err("legacy and vault basic must conflict");

        assert_eq!(error.code(), "INVALID_ARGUMENT");
        assert!(!format!("{error:?}").contains("http-conflict-sentinel"));
    }

    #[test]
    fn resolved_basic_auth_builds_internal_credentials_with_redacted_debug() {
        let auth = parse_auth(
            None,
            None,
            Some(BasicCredential::new("vault-user", "http-boundary-sentinel")),
        )
        .expect("resolved Basic auth should parse");

        let AuthConfig::Basic { username, password } = &auth else {
            panic!("expected Basic auth")
        };
        assert_eq!(username, "vault-user");
        assert_eq!(password, "http-boundary-sentinel");
        assert!(!format!("{auth:?}").contains("http-boundary-sentinel"));
    }
}
