use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::AppError, output::OutputMode};

use super::{
    AssertArgs, AssertReportArg, BasicCredential, MethodShortcutArgs, ReplayArgs, RequestArgs,
    RequestExpectArgs, RequestOptionsArgs, adapters,
};

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

#[derive(Debug, Default)]
struct RequestExpectations {
    status: Option<StatusExpectation>,
    headers: Vec<(String, String)>,
    body_contains: Vec<String>,
    json: Vec<JsonExpectation>,
}

#[derive(Debug, Clone)]
enum StatusExpectation {
    Exact(u16),
    Class(u16),
    Range(u16, u16),
}

#[derive(Debug, Clone)]
struct JsonExpectation {
    path: String,
    operator: JsonExpectationOperator,
    source: String,
}

#[derive(Debug, Clone)]
enum JsonExpectationOperator {
    Eq(Value),
    Contains(Value),
    Exists(bool),
    Match(Regex),
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

#[derive(Debug, Default, Serialize)]
pub(crate) struct AssertionSummary {
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) failures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpRequestOutput {
    pub(crate) command: String,
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) status: u16,
    #[serde(skip)]
    pub(crate) status_text: String,
    pub(crate) ok: bool,
    pub(crate) duration_ms: u64,
    pub(crate) truncated: bool,
    pub(crate) body_truncated: bool,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: String,
    pub(crate) assertions: AssertionSummary,
}

#[derive(Debug, Deserialize)]
struct HttpSpec {
    version: u32,
    #[serde(default)]
    defaults: SpecDefaults,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    cases: Vec<SpecCase>,
}

#[derive(Debug, Default, Deserialize)]
struct SpecDefaults {
    base_url: Option<String>,
    timeout_secs: Option<u64>,
    max_response_bytes: Option<usize>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    query: BTreeMap<String, String>,
    bearer: Option<String>,
    basic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SpecCase {
    name: String,
    request: SpecRequest,
    #[serde(default)]
    expect: SpecExpect,
    #[serde(default)]
    extract: BTreeMap<String, SpecExtractRule>,
}

#[derive(Debug, Default, Deserialize)]
struct SpecRequest {
    method: Option<String>,
    path: Option<String>,
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    query: BTreeMap<String, String>,
    timeout_secs: Option<u64>,
    max_response_bytes: Option<usize>,
    bearer: Option<String>,
    basic: Option<String>,
    json: Option<Value>,
    json_file: Option<String>,
    body: Option<String>,
    body_file: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SpecExpect {
    status: Option<SpecStatusValue>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body_contains: Option<OneOrManyStrings>,
    #[serde(default)]
    json: Vec<SpecJsonCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SpecStatusValue {
    Number(u16),
    Text(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrManyStrings {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Deserialize)]
struct SpecJsonCheck {
    path: String,
    eq: Option<Value>,
    contains: Option<Value>,
    exists: Option<bool>,
    #[serde(rename = "match")]
    regex: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecExtractRule {
    json: Option<String>,
    header: Option<String>,
    text: Option<SpecTextExtract>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecTextExtract {
    regex: String,
    #[serde(default = "default_extract_group")]
    group: usize,
}

fn default_extract_group() -> usize {
    1
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpAssertOutput {
    pub(crate) command: &'static str,
    pub(crate) spec_path: String,
    pub(crate) fail_fast: bool,
    pub(crate) summary: HttpAssertSummary,
    pub(crate) cases: Vec<HttpAssertCaseOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpAssertSummary {
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct HttpAssertCaseOutput {
    pub(crate) name: String,
    pub(crate) passed: bool,
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

pub(crate) fn run_assert(
    args: AssertArgs,
    output: OutputMode,
    _command_name: &'static str,
) -> Result<(HttpAssertOutput, AssertReportFormat), AppError> {
    let report_format = resolve_assert_report_mode(output, args.report)?;
    let spec_path = args.spec_path;
    let spec_dir = spec_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let spec = read_spec_file(&spec_path)?;

    if spec.version != 1 {
        return Err(AppError::invalid_argument(format!(
            "unsupported spec version {} (expected 1)",
            spec.version
        )));
    }
    if spec.cases.is_empty() {
        return Err(AppError::invalid_argument("spec has no cases"));
    }
    for case in &spec.cases {
        parse_extractors(&case.extract)?;
    }

    let mut vars = spec.vars;
    for pair in &args.vars {
        let (key, value) = parse_key_value_pair(pair, "--var", '=')?;
        vars.insert(key, value);
    }

    let started = std::time::Instant::now();
    let mut cases = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for case in &spec.cases {
        let case_started = std::time::Instant::now();
        let prepared = build_case_request(case, &spec.defaults, &vars, &spec_dir)?;
        let response = send_with_retry(
            &prepared.request,
            args.retry,
            args.retry_delay_ms,
            args.deadline,
        )?;
        let assertions = evaluate_assertions(&response, &prepared.expectations);
        let (extracted, mut extraction_failures) =
            extract_response_values(&response, &prepared.extractors);
        let mut failures = assertions.failures;
        failures.append(&mut extraction_failures);
        let case_passed = failures.is_empty();
        if case_passed {
            passed += 1;
            vars.extend(extracted);
        } else {
            failed += 1;
        }
        cases.push(HttpAssertCaseOutput {
            name: prepared.case_name,
            passed: case_passed,
            status: Some(response.status_code),
            duration_ms: duration_millis(case_started.elapsed()),
            failures,
        });
        if args.fail_fast && !case_passed {
            break;
        }
    }

    let output = HttpAssertOutput {
        command: "http.assert",
        spec_path: normalize_path(&spec_path),
        fail_fast: args.fail_fast,
        summary: HttpAssertSummary {
            total: cases.len(),
            passed,
            failed,
            duration_ms: duration_millis(started.elapsed()),
        },
        cases,
    };

    Ok((output, report_format))
}

fn resolve_assert_report_mode(
    output: OutputMode,
    report: Option<AssertReportArg>,
) -> Result<AssertReportFormat, AppError> {
    match (output, report) {
        (OutputMode::Text, None) => Ok(AssertReportFormat::Text),
        (OutputMode::Text, Some(AssertReportArg::Text)) => Ok(AssertReportFormat::Text),
        (OutputMode::Text, Some(AssertReportArg::Json)) => Ok(AssertReportFormat::Json),
        (OutputMode::Text, Some(AssertReportArg::Junit)) => Ok(AssertReportFormat::Junit),
        (OutputMode::Json, None) => Ok(AssertReportFormat::Json),
        (OutputMode::Json, Some(AssertReportArg::Json)) => Ok(AssertReportFormat::Json),
        (OutputMode::Json, Some(AssertReportArg::Text | AssertReportArg::Junit)) => Err(
            AppError::invalid_argument("--json conflicts with --report (use --report json)"),
        ),
    }
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

fn parse_payload(
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

fn parse_request_expectations(args: &RequestExpectArgs) -> Result<RequestExpectations, AppError> {
    let status = args
        .expect_status
        .as_ref()
        .map(|value| parse_status_expectation(value))
        .transpose()?;

    let mut headers = Vec::new();
    for raw in &args.expect_headers {
        let (name, value) = parse_header(raw, "--expect-header")?;
        headers.push((name.to_ascii_lowercase(), value));
    }

    let mut json = Vec::new();
    for raw in &args.expect_json {
        json.push(parse_json_expectation_expression(raw)?);
    }

    Ok(RequestExpectations {
        status,
        headers,
        body_contains: args.expect_body_contains.clone(),
        json,
    })
}

fn evaluate_assertions(
    response: &ResponseSnapshot,
    expectations: &RequestExpectations,
) -> AssertionSummary {
    let mut summary = AssertionSummary::default();

    if let Some(status_expectation) = &expectations.status {
        summary.total += 1;
        if status_expectation.matches(response.status_code) {
            summary.passed += 1;
        } else {
            summary.failed += 1;
            summary.failures.push(format!(
                "status expected {}, got {}",
                status_expectation.describe(),
                response.status_code
            ));
        }
    }

    for (name, expected_value) in &expectations.headers {
        summary.total += 1;
        match response.headers.get(name) {
            Some(actual_value) if actual_value.trim() == expected_value.trim() => {
                summary.passed += 1;
            }
            Some(actual_value) => {
                summary.failed += 1;
                summary.failures.push(format!(
                    "header '{name}' expected '{}', got '{}'",
                    expected_value, actual_value
                ));
            }
            None => {
                summary.failed += 1;
                summary.failures.push(format!(
                    "header '{name}' expected '{}', but was missing",
                    expected_value
                ));
            }
        }
    }

    for expected in &expectations.body_contains {
        summary.total += 1;
        if response.body_truncated {
            summary.failed += 1;
            summary.failures.push(format!(
                "response body was truncated; cannot evaluate body contains '{}'",
                expected
            ));
        } else if response.body.contains(expected) {
            summary.passed += 1;
        } else {
            summary.failed += 1;
            summary
                .failures
                .push(format!("body does not contain '{}'", expected));
        }
    }

    for expectation in &expectations.json {
        summary.total += 1;
        if response.body_truncated {
            summary.failed += 1;
            summary.failures.push(format!(
                "response body was truncated; cannot evaluate json expectation: {}",
                expectation.source
            ));
            continue;
        }
        match &response.body_json {
            Some(json) => {
                if evaluate_json_expectation(json, expectation) {
                    summary.passed += 1;
                } else {
                    summary.failed += 1;
                    summary
                        .failures
                        .push(format!("json expectation failed: {}", expectation.source));
                }
            }
            None => {
                summary.failed += 1;
                summary
                    .failures
                    .push("json expectation failed: response body is not valid JSON".to_owned());
            }
        }
    }

    summary
}

fn evaluate_json_expectation(root: &Value, expectation: &JsonExpectation) -> bool {
    let actual = resolve_json_path(root, &expectation.path);
    match &expectation.operator {
        JsonExpectationOperator::Eq(expected) => actual == Some(expected),
        JsonExpectationOperator::Contains(expected) => match (actual, expected) {
            (Some(Value::String(actual_text)), Value::String(expected_text)) => {
                actual_text.contains(expected_text)
            }
            (Some(Value::Array(items)), _) => items.iter().any(|item| item == expected),
            _ => false,
        },
        JsonExpectationOperator::Exists(expected_exists) => actual.is_some() == *expected_exists,
        JsonExpectationOperator::Match(regex) => match actual {
            Some(Value::String(actual_text)) => regex.is_match(actual_text),
            _ => false,
        },
    }
}

fn parse_status_expectation(raw: &str) -> Result<StatusExpectation, AppError> {
    let value = raw.trim().to_ascii_lowercase();
    if value.is_empty() {
        return Err(AppError::invalid_argument(
            "status expectation must not be empty",
        ));
    }

    if let Ok(code) = value.parse::<u16>() {
        if !(100..=599).contains(&code) {
            return Err(AppError::invalid_argument(format!(
                "status code out of range: {code}"
            )));
        }
        return Ok(StatusExpectation::Exact(code));
    }

    if value.len() == 3
        && value.ends_with("xx")
        && value
            .chars()
            .next()
            .is_some_and(|ch| ('1'..='5').contains(&ch))
    {
        let class = value
            .chars()
            .next()
            .and_then(|ch| ch.to_digit(10))
            .map(|digit| digit as u16)
            .ok_or_else(|| AppError::invalid_argument(format!("invalid status class: {raw}")))?;
        return Ok(StatusExpectation::Class(class));
    }

    if let Some((left, right)) = value.split_once('-') {
        let start = left.parse::<u16>().map_err(|error| {
            AppError::invalid_argument(format!("invalid status range '{raw}': {error}"))
        })?;
        let end = right.parse::<u16>().map_err(|error| {
            AppError::invalid_argument(format!("invalid status range '{raw}': {error}"))
        })?;
        if start > end {
            return Err(AppError::invalid_argument(format!(
                "invalid status range '{raw}': start must be <= end"
            )));
        }
        return Ok(StatusExpectation::Range(start, end));
    }

    Err(AppError::invalid_argument(format!(
        "invalid status expectation '{raw}' (use 200, 2xx, or 200-299)"
    )))
}

fn parse_json_expectation_expression(raw: &str) -> Result<JsonExpectation, AppError> {
    let mut parts = raw.splitn(3, ':');
    let path = parts.next().unwrap_or_default().trim();
    let operator = parts.next().unwrap_or_default().trim();
    let remainder = parts.next().map(str::trim);

    if path.is_empty() || operator.is_empty() {
        return Err(AppError::invalid_argument(format!(
            "invalid --expect-json '{raw}' (expected PATH:OP[:VALUE])"
        )));
    }

    let parsed_operator = match operator {
        "eq" => {
            let value = remainder.ok_or_else(|| {
                AppError::invalid_argument(format!(
                    "invalid --expect-json '{raw}': eq requires value"
                ))
            })?;
            JsonExpectationOperator::Eq(parse_json_literal_or_string(value))
        }
        "contains" => {
            let value = remainder.ok_or_else(|| {
                AppError::invalid_argument(format!(
                    "invalid --expect-json '{raw}': contains requires value"
                ))
            })?;
            JsonExpectationOperator::Contains(parse_json_literal_or_string(value))
        }
        "exists" => {
            let expected = match remainder {
                None => true,
                Some("true") => true,
                Some("false") => false,
                Some(other) => {
                    return Err(AppError::invalid_argument(format!(
                        "invalid --expect-json '{raw}': exists value must be true or false, got '{other}'"
                    )));
                }
            };
            JsonExpectationOperator::Exists(expected)
        }
        "match" => {
            let value = remainder.ok_or_else(|| {
                AppError::invalid_argument(format!(
                    "invalid --expect-json '{raw}': match requires regex value"
                ))
            })?;
            let regex = Regex::new(value).map_err(|error| {
                AppError::invalid_argument(format!(
                    "invalid --expect-json '{raw}': regex is invalid: {error}"
                ))
            })?;
            JsonExpectationOperator::Match(regex)
        }
        _ => {
            return Err(AppError::invalid_argument(format!(
                "invalid --expect-json '{raw}': unsupported operator '{operator}'"
            )));
        }
    };

    Ok(JsonExpectation {
        path: path.to_owned(),
        operator: parsed_operator,
        source: raw.to_owned(),
    })
}

fn resolve_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if path.trim().is_empty() {
        return None;
    }

    let mut current = value;
    for token in parse_json_path_tokens(path).ok()? {
        current = match token {
            JsonPathToken::Key(key) => current.get(key.as_str())?,
            JsonPathToken::Index(index) => current.get(index)?,
        };
    }
    Some(current)
}

#[derive(Debug)]
enum JsonPathToken {
    Key(String),
    Index(usize),
}

fn parse_json_path_tokens(path: &str) -> Result<Vec<JsonPathToken>, AppError> {
    let mut tokens = Vec::new();
    for part in path.split('.') {
        if part.trim().is_empty() {
            return Err(AppError::invalid_argument(format!(
                "invalid json path '{path}'"
            )));
        }
        parse_json_path_part(part, &mut tokens)?;
    }
    Ok(tokens)
}

fn parse_json_path_part(part: &str, out: &mut Vec<JsonPathToken>) -> Result<(), AppError> {
    let mut remaining = part;
    if let Some(index_start) = remaining.find('[') {
        if index_start > 0 {
            out.push(JsonPathToken::Key(remaining[..index_start].to_owned()));
        }
        remaining = &remaining[index_start..];
    } else {
        out.push(JsonPathToken::Key(remaining.to_owned()));
        return Ok(());
    }

    while !remaining.is_empty() {
        if !remaining.starts_with('[') {
            return Err(AppError::invalid_argument(format!(
                "invalid json path segment '{part}'"
            )));
        }
        let close = remaining.find(']').ok_or_else(|| {
            AppError::invalid_argument(format!("invalid json path segment '{part}'"))
        })?;
        let index_text = &remaining[1..close];
        let index = index_text.parse::<usize>().map_err(|_| {
            AppError::invalid_argument(format!("invalid json path segment '{part}'"))
        })?;
        out.push(JsonPathToken::Index(index));
        remaining = &remaining[(close + 1)..];
    }
    Ok(())
}

fn parse_json_literal_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

impl StatusExpectation {
    fn matches(&self, code: u16) -> bool {
        match self {
            StatusExpectation::Exact(value) => code == *value,
            StatusExpectation::Class(class) => code / 100 == *class,
            StatusExpectation::Range(start, end) => code >= *start && code <= *end,
        }
    }

    fn describe(&self) -> String {
        match self {
            StatusExpectation::Exact(value) => value.to_string(),
            StatusExpectation::Class(class) => format!("{class}xx"),
            StatusExpectation::Range(start, end) => format!("{start}-{end}"),
        }
    }
}

fn parse_auth(
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

fn parse_basic_auth(raw: &str) -> Result<(String, String), AppError> {
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

fn parse_header(raw: &str, flag_name: &str) -> Result<(String, String), AppError> {
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

fn normalize_method(raw: &str) -> Result<String, AppError> {
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

#[derive(Debug)]
struct ParsedCurlReplay {
    method: Option<String>,
    url: String,
    headers: Vec<(String, String)>,
    timeout_secs: Option<u64>,
    bearer: Option<String>,
    basic: Option<String>,
    body: Option<RequestBody>,
}

fn parse_curl_replay(raw: &str) -> Result<ParsedCurlReplay, AppError> {
    let mut tokens = shell_words::split(raw)
        .map_err(|error| AppError::invalid_argument(format!("failed to parse --curl: {error}")))?;
    if tokens.is_empty() {
        return Err(AppError::invalid_argument(
            "--curl command must not be empty",
        ));
    }
    if tokens[0] == "curl" {
        tokens.remove(0);
    }
    if tokens.is_empty() {
        return Err(AppError::invalid_argument(
            "--curl command must include URL",
        ));
    }

    let mut method = None;
    let mut url = None;
    let mut headers = Vec::new();
    let mut timeout_secs = None;
    let mut bearer = None;
    let mut basic = None;
    let mut body = None;

    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "-X" || token == "--request" {
            index += 1;
            let value = tokens.get(index).ok_or_else(|| {
                AppError::invalid_argument("curl option --request requires value")
            })?;
            method = Some(normalize_method(value)?);
        } else if let Some(value) = token.strip_prefix("--request=") {
            method = Some(normalize_method(value)?);
        } else if token == "-H" || token == "--header" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| AppError::invalid_argument("curl option --header requires value"))?;
            headers.push(parse_header(value, "--header")?);
        } else if let Some(value) = token.strip_prefix("--header=") {
            headers.push(parse_header(value, "--header")?);
        } else if matches!(
            token.as_str(),
            "-d" | "--data" | "--data-raw" | "--data-binary"
        ) {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| AppError::invalid_argument("curl data option requires value"))?;
            body = Some(RequestBody::Text(value.clone()));
            if method.is_none() {
                method = Some("POST".to_owned());
            }
        } else if token.starts_with("--data=")
            || token.starts_with("--data-raw=")
            || token.starts_with("--data-binary=")
        {
            let value = token
                .split_once('=')
                .map(|(_, right)| right)
                .unwrap_or_default();
            body = Some(RequestBody::Text(value.to_owned()));
            if method.is_none() {
                method = Some("POST".to_owned());
            }
        } else if token == "--json" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| AppError::invalid_argument("curl option --json requires value"))?;
            let json = serde_json::from_str::<Value>(value).map_err(|error| {
                AppError::invalid_argument(format!("curl --json value is invalid JSON: {error}"))
            })?;
            body = Some(RequestBody::Json(json));
            if method.is_none() {
                method = Some("POST".to_owned());
            }
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        } else if let Some(value) = token.strip_prefix("--json=") {
            let json = serde_json::from_str::<Value>(value).map_err(|error| {
                AppError::invalid_argument(format!("curl --json value is invalid JSON: {error}"))
            })?;
            body = Some(RequestBody::Json(json));
            if method.is_none() {
                method = Some("POST".to_owned());
            }
            headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
        } else if token == "-u" || token == "--user" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| AppError::invalid_argument("curl option --user requires value"))?;
            parse_basic_auth(value)?;
            basic = Some(value.clone());
        } else if let Some(value) = token.strip_prefix("--user=") {
            parse_basic_auth(value)?;
            basic = Some(value.to_owned());
        } else if token == "-m" || token == "--max-time" {
            index += 1;
            let value = tokens.get(index).ok_or_else(|| {
                AppError::invalid_argument("curl option --max-time requires value")
            })?;
            let seconds = parse_timeout_secs(value, "--max-time")?;
            timeout_secs = Some(seconds);
        } else if let Some(value) = token.strip_prefix("--max-time=") {
            let seconds = parse_timeout_secs(value, "--max-time")?;
            timeout_secs = Some(seconds);
        } else if token == "--url" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| AppError::invalid_argument("curl option --url requires value"))?;
            url = Some(value.clone());
        } else if let Some(value) = token.strip_prefix("--url=") {
            url = Some(value.to_owned());
        } else if token == "-I" || token == "--head" {
            method = Some("HEAD".to_owned());
        } else if token == "--get" {
            method = Some("GET".to_owned());
        } else if token.starts_with("http://") || token.starts_with("https://") {
            url = Some(token.clone());
        } else if token.starts_with("--") || token.starts_with('-') {
            return Err(AppError::invalid_argument(format!(
                "unsupported curl option in replay: {token}"
            )));
        } else {
            url = Some(token.clone());
        }

        index += 1;
    }

    let mut normalized_headers = Vec::new();
    let mut bearer_token = None;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("authorization")
            && value.to_ascii_lowercase().starts_with("bearer ")
        {
            let token = value[7..].trim().to_owned();
            if !token.is_empty() {
                bearer_token = Some(token);
            }
        }
        normalized_headers.push((name, value));
    }
    if bearer.is_none() {
        bearer = bearer_token;
    }

    Ok(ParsedCurlReplay {
        method,
        url: url.ok_or_else(|| AppError::invalid_argument("curl command must include URL"))?,
        headers: normalized_headers,
        timeout_secs,
        bearer,
        basic,
        body,
    })
}

fn parse_timeout_secs(raw: &str, flag_name: &str) -> Result<u64, AppError> {
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

struct PreparedSpecCase {
    case_name: String,
    request: RequestConfig,
    expectations: RequestExpectations,
    extractors: BTreeMap<String, Extractor>,
}

enum Extractor {
    Json { path: String },
    Header { name: String },
    Text { regex: Regex, group: usize },
}

fn build_case_request(
    case: &SpecCase,
    defaults: &SpecDefaults,
    vars: &BTreeMap<String, String>,
    spec_dir: &Path,
) -> Result<PreparedSpecCase, AppError> {
    if case.name.trim().is_empty() {
        return Err(AppError::invalid_argument("case name must not be empty"));
    }

    let method = case
        .request
        .method
        .as_deref()
        .map(normalize_method)
        .transpose()?
        .unwrap_or_else(|| "GET".to_owned());

    let url = resolve_case_url(&case.request, defaults, vars)?;

    let mut headers = Vec::new();
    for (name, value) in &defaults.headers {
        headers.push((
            interpolate_string(name, vars)?,
            interpolate_string(value, vars)?,
        ));
    }
    for (name, value) in &case.request.headers {
        headers.push((
            interpolate_string(name, vars)?,
            interpolate_string(value, vars)?,
        ));
    }

    let mut query = Vec::new();
    for (name, value) in &defaults.query {
        query.push((
            interpolate_string(name, vars)?,
            interpolate_string(value, vars)?,
        ));
    }
    for (name, value) in &case.request.query {
        query.push((
            interpolate_string(name, vars)?,
            interpolate_string(value, vars)?,
        ));
    }

    let timeout_secs = case
        .request
        .timeout_secs
        .or(defaults.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .max(1);
    let max_response_bytes = case
        .request
        .max_response_bytes
        .or(defaults.max_response_bytes)
        .unwrap_or(DEFAULT_MAX_RESPONSE_BYTES);
    if max_response_bytes == 0 {
        return Err(AppError::invalid_argument(format!(
            "case '{}' max_response_bytes must be >= 1",
            case.name
        )));
    }

    let bearer = case
        .request
        .bearer
        .as_ref()
        .or(defaults.bearer.as_ref())
        .map(|value| interpolate_string(value, vars))
        .transpose()?;
    let basic = case
        .request
        .basic
        .as_ref()
        .or(defaults.basic.as_ref())
        .map(|value| interpolate_string(value, vars))
        .transpose()?;
    let auth = parse_auth(bearer.as_deref(), basic.as_deref(), None)?;

    let request_options = RequestOptionsArgs {
        headers: Vec::new(),
        query: Vec::new(),
        timeout_secs: Some(timeout_secs),
        max_response_bytes: Some(max_response_bytes),
        retry: 0,
        retry_delay_ms: 0,
        bearer,
        basic,
        resolved_basic: None,
        json: None,
        json_file: None,
        body: None,
        body_file: None,
        deadline: None,
    };
    let body = parse_spec_payload(&case.request, vars, spec_dir, &request_options)?;

    let request = RequestConfig {
        method,
        url,
        headers,
        query,
        timeout: Duration::from_secs(timeout_secs),
        max_response_bytes,
        auth,
        body,
    };

    let expectations = parse_spec_expectations(&case.expect, vars)?;
    let extractors = parse_extractors(&case.extract)?;

    Ok(PreparedSpecCase {
        case_name: interpolate_string(&case.name, vars)?,
        request,
        expectations,
        extractors,
    })
}

fn parse_extractors(
    rules: &BTreeMap<String, SpecExtractRule>,
) -> Result<BTreeMap<String, Extractor>, AppError> {
    let mut extractors = BTreeMap::new();
    for (variable, rule) in rules {
        if variable.trim().is_empty() {
            return Err(AppError::invalid_argument(
                "extract variable name must not be empty",
            ));
        }
        let selector_count = usize::from(rule.json.is_some())
            + usize::from(rule.header.is_some())
            + usize::from(rule.text.is_some());
        if selector_count != 1 {
            return Err(AppError::invalid_argument(format!(
                "extract '{variable}' must define exactly one selector (json, header, text)"
            )));
        }
        let extractor = if let Some(path) = &rule.json {
            parse_json_path_tokens(path)?;
            Extractor::Json { path: path.clone() }
        } else if let Some(name) = &rule.header {
            if name.trim().is_empty() {
                return Err(AppError::invalid_argument(format!(
                    "extract '{variable}' header must not be empty"
                )));
            }
            Extractor::Header {
                name: name.to_ascii_lowercase(),
            }
        } else {
            let text = rule.text.as_ref().expect("selector count checked");
            let regex = Regex::new(&text.regex).map_err(|error| {
                AppError::invalid_argument(format!(
                    "extract '{variable}' has invalid text regex: {error}"
                ))
            })?;
            if text.group >= regex.captures_len() {
                return Err(AppError::invalid_argument(format!(
                    "extract '{variable}' group {} does not exist in text regex",
                    text.group
                )));
            }
            Extractor::Text {
                regex,
                group: text.group,
            }
        };
        extractors.insert(variable.clone(), extractor);
    }
    Ok(extractors)
}

fn extract_response_values(
    response: &ResponseSnapshot,
    extractors: &BTreeMap<String, Extractor>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut failures = Vec::new();

    for (variable, extractor) in extractors {
        match extract_response_value(response, extractor) {
            Ok(value) => {
                values.insert(variable.clone(), value);
            }
            Err(source) => failures.push(format!("extract '{variable}' failed: {source}")),
        }
    }
    if failures.is_empty() {
        (values, failures)
    } else {
        (BTreeMap::new(), failures)
    }
}

fn extract_response_value(
    response: &ResponseSnapshot,
    extractor: &Extractor,
) -> Result<String, &'static str> {
    match extractor {
        Extractor::Json { path } => {
            if response.body_truncated {
                return Err("response body was truncated");
            }
            let root = response
                .body_json
                .as_ref()
                .ok_or("response body is not valid JSON")?;
            let value = resolve_json_path(root, path).ok_or("JSON path was not found")?;
            match value {
                Value::String(value) => Ok(value.clone()),
                value => Ok(value.to_string()),
            }
        }
        Extractor::Header { name } => response
            .headers
            .get(name)
            .cloned()
            .ok_or("response header was not found"),
        Extractor::Text { regex, group } => {
            if response.body_truncated {
                return Err("response body was truncated");
            }
            regex
                .captures(&response.body)
                .and_then(|captures| captures.get(*group))
                .map(|capture| capture.as_str().to_owned())
                .ok_or("text regex did not match the requested group")
        }
    }
}

fn parse_spec_expectations(
    expect: &SpecExpect,
    vars: &BTreeMap<String, String>,
) -> Result<RequestExpectations, AppError> {
    let status = expect
        .status
        .as_ref()
        .map(|value| match value {
            SpecStatusValue::Number(code) => parse_status_expectation(&code.to_string()),
            SpecStatusValue::Text(text) => parse_status_expectation(text),
        })
        .transpose()?;

    let mut headers = Vec::new();
    for (name, value) in &expect.headers {
        headers.push((
            interpolate_string(name, vars)?.to_ascii_lowercase(),
            interpolate_string(value, vars)?,
        ));
    }

    let mut body_contains = Vec::new();
    match &expect.body_contains {
        Some(OneOrManyStrings::One(value)) => body_contains.push(interpolate_string(value, vars)?),
        Some(OneOrManyStrings::Many(values)) => {
            for value in values {
                body_contains.push(interpolate_string(value, vars)?);
            }
        }
        None => {}
    }

    let mut json = Vec::new();
    for check in &expect.json {
        json.push(parse_spec_json_check(check, vars)?);
    }

    Ok(RequestExpectations {
        status,
        headers,
        body_contains,
        json,
    })
}

fn parse_spec_json_check(
    check: &SpecJsonCheck,
    vars: &BTreeMap<String, String>,
) -> Result<JsonExpectation, AppError> {
    let path = interpolate_string(&check.path, vars)?;
    let mut operators = 0usize;
    if check.eq.is_some() {
        operators += 1;
    }
    if check.contains.is_some() {
        operators += 1;
    }
    if check.exists.is_some() {
        operators += 1;
    }
    if check.regex.is_some() {
        operators += 1;
    }
    if operators != 1 {
        return Err(AppError::invalid_argument(format!(
            "json check for path '{}' must define exactly one operator (eq, contains, exists, match)",
            path
        )));
    }

    if let Some(value) = &check.eq {
        let expected = interpolate_json_value(value, vars)?;
        return Ok(JsonExpectation {
            source: format!("{path}:eq:{expected}"),
            path,
            operator: JsonExpectationOperator::Eq(expected),
        });
    }
    if let Some(value) = &check.contains {
        let expected = interpolate_json_value(value, vars)?;
        return Ok(JsonExpectation {
            source: format!("{path}:contains:{expected}"),
            path,
            operator: JsonExpectationOperator::Contains(expected),
        });
    }
    if let Some(value) = check.exists {
        return Ok(JsonExpectation {
            source: format!("{path}:exists:{value}"),
            path,
            operator: JsonExpectationOperator::Exists(value),
        });
    }

    let regex_text = interpolate_string(check.regex.as_deref().unwrap_or_default(), vars)?;
    let regex = Regex::new(&regex_text).map_err(|error| {
        AppError::invalid_argument(format!(
            "invalid regex for json check '{}': {error}",
            regex_text
        ))
    })?;
    Ok(JsonExpectation {
        source: format!("{path}:match:{regex_text}"),
        path,
        operator: JsonExpectationOperator::Match(regex),
    })
}

fn parse_spec_payload(
    request: &SpecRequest,
    vars: &BTreeMap<String, String>,
    spec_dir: &Path,
    defaults: &RequestOptionsArgs,
) -> Result<Option<RequestBody>, AppError> {
    let mut options = defaults.clone();
    options.json = None;
    options.json_file = None;
    options.body = None;
    options.body_file = None;

    if let Some(json) = &request.json {
        options.json = Some(interpolate_json_value(json, vars)?.to_string());
    }
    if let Some(json_file) = &request.json_file {
        options.json_file = Some(resolve_spec_relative_path(
            spec_dir,
            &interpolate_string(json_file, vars)?,
        ));
    }
    if let Some(body) = &request.body {
        options.body = Some(interpolate_string(body, vars)?);
    }
    if let Some(body_file) = &request.body_file {
        options.body_file = Some(resolve_spec_relative_path(
            spec_dir,
            &interpolate_string(body_file, vars)?,
        ));
    }

    parse_payload(&options, Some(spec_dir))
}

fn resolve_case_url(
    request: &SpecRequest,
    defaults: &SpecDefaults,
    vars: &BTreeMap<String, String>,
) -> Result<String, AppError> {
    if request.url.is_some() && request.path.is_some() {
        return Err(AppError::invalid_argument(
            "request.url and request.path are mutually exclusive",
        ));
    }

    if let Some(url) = request.url.as_ref() {
        let interpolated = interpolate_string(url, vars)?;
        if interpolated.trim().is_empty() {
            return Err(AppError::invalid_argument("request.url must not be empty"));
        }
        return Ok(interpolated);
    }

    if let Some(path) = request.path.as_ref() {
        let base = defaults
            .base_url
            .as_ref()
            .ok_or_else(|| AppError::invalid_argument("request.path requires defaults.base_url"))?;
        let base = interpolate_string(base, vars)?;
        let path = interpolate_string(path, vars)?;
        if path.starts_with("http://") || path.starts_with("https://") {
            return Ok(path);
        }
        return Ok(join_base_and_path(&base, &path));
    }

    Err(AppError::invalid_argument(
        "request must define either url or path",
    ))
}

fn join_base_and_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

fn read_spec_file(path: &Path) -> Result<HttpSpec, AppError> {
    let raw = adapters::io::read_to_string(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    if extension == "json" {
        serde_json::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse spec JSON '{}': {error}",
                path.display()
            ))
        })
    } else {
        serde_yaml::from_str(&raw).map_err(|error| {
            AppError::invalid_argument(format!(
                "failed to parse spec YAML '{}': {error}",
                path.display()
            ))
        })
    }
}

fn interpolate_string(input: &str, vars: &BTreeMap<String, String>) -> Result<String, AppError> {
    let mut remaining = input;
    let mut output = String::with_capacity(input.len());

    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[(start + 2)..];
        let end = after_start.find("}}").ok_or_else(|| {
            AppError::invalid_argument(format!("unterminated template expression in '{input}'"))
        })?;
        let key = after_start[..end].trim();
        if key.is_empty() {
            return Err(AppError::invalid_argument(format!(
                "empty template expression in '{input}'"
            )));
        }
        let value = vars.get(key).ok_or_else(|| {
            AppError::invalid_argument(format!("unknown template variable '{key}'"))
        })?;
        output.push_str(value);
        remaining = &after_start[(end + 2)..];
    }

    output.push_str(remaining);
    Ok(output)
}

fn interpolate_json_value(
    value: &Value,
    vars: &BTreeMap<String, String>,
) -> Result<Value, AppError> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Bool(boolean) => Ok(Value::Bool(*boolean)),
        Value::Number(number) => Ok(Value::Number(number.clone())),
        Value::String(text) => Ok(Value::String(interpolate_string(text, vars)?)),
        Value::Array(items) => {
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                result.push(interpolate_json_value(item, vars)?);
            }
            Ok(Value::Array(result))
        }
        Value::Object(map) => {
            let mut result = serde_json::Map::new();
            for (key, item) in map {
                let new_key = interpolate_string(key, vars)?;
                result.insert(new_key, interpolate_json_value(item, vars)?);
            }
            Ok(Value::Object(result))
        }
    }
}

pub(crate) fn render_assert_junit(report: &HttpAssertOutput) -> String {
    let mut xml = String::new();
    xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    xml.push('\n');
    xml.push_str(&format!(
        r#"<testsuite name="http.assert" tests="{}" failures="{}" time="{}">"#,
        report.summary.total,
        report.summary.failed,
        duration_secs_string(report.summary.duration_ms)
    ));
    xml.push('\n');
    for case in &report.cases {
        xml.push_str(&format!(
            r#"  <testcase name="{}" classname="http.assert" time="{}">"#,
            xml_escape(&case.name),
            duration_secs_string(case.duration_ms)
        ));
        xml.push('\n');
        if !case.passed {
            let message = case.failures.join("; ");
            xml.push_str(&format!(
                r#"    <failure message="{}">{}</failure>"#,
                xml_escape(&message),
                xml_escape(&message)
            ));
            xml.push('\n');
        }
        xml.push_str("  </testcase>\n");
    }
    xml.push_str("</testsuite>");
    xml
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn duration_secs_string(duration_ms: u64) -> String {
    format!("{:.3}", (duration_ms as f64) / 1000.0)
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

fn resolve_spec_relative_path(base_dir: &Path, value: &str) -> PathBuf {
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

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_expectation_parses_formats() {
        assert!(matches!(
            parse_status_expectation("200").expect("exact"),
            StatusExpectation::Exact(200)
        ));
        assert!(matches!(
            parse_status_expectation("2xx").expect("class"),
            StatusExpectation::Class(2)
        ));
        assert!(matches!(
            parse_status_expectation("200-299").expect("range"),
            StatusExpectation::Range(200, 299)
        ));
    }

    #[test]
    fn json_expectation_expression_parses() {
        let expectation =
            parse_json_expectation_expression("data.user.id:eq:42").expect("expression parses");
        assert_eq!(expectation.path, "data.user.id");
    }

    #[test]
    fn json_path_supports_arrays() {
        let value: Value = serde_json::json!({
            "items": [{"name":"first"}, {"name":"second"}]
        });
        let resolved = resolve_json_path(&value, "items[1].name").expect("path should resolve");
        assert_eq!(resolved, "second");
    }

    #[test]
    fn interpolation_replaces_placeholders() {
        let mut vars = BTreeMap::new();
        vars.insert("base".to_owned(), "http://localhost:8080".to_owned());
        let rendered =
            interpolate_string("{{base}}/health", &vars).expect("template should render");
        assert_eq!(rendered, "http://localhost:8080/health");
    }

    #[test]
    fn extraction_is_atomic_when_one_selector_fails() {
        let response = ResponseSnapshot {
            status_code: 200,
            status_text: "OK".to_owned(),
            headers: BTreeMap::new(),
            body: r#"{"token":"secret"}"#.to_owned(),
            body_json: Some(serde_json::json!({"token": "secret"})),
            body_truncated: false,
        };
        let extractors = BTreeMap::from([
            (
                "token".to_owned(),
                Extractor::Json {
                    path: "token".to_owned(),
                },
            ),
            (
                "request_id".to_owned(),
                Extractor::Header {
                    name: "x-request-id".to_owned(),
                },
            ),
        ]);

        let (values, failures) = extract_response_values(&response, &extractors);

        assert!(values.is_empty());
        assert_eq!(failures.len(), 1);
        assert!(!failures[0].contains("secret"));
    }

    #[test]
    fn text_extraction_rejects_truncated_bodies() {
        let response = ResponseSnapshot {
            status_code: 200,
            status_text: "OK".to_owned(),
            headers: BTreeMap::new(),
            body: "token=secret".to_owned(),
            body_json: None,
            body_truncated: true,
        };
        let extractor = Extractor::Text {
            regex: Regex::new("token=(.+)").expect("regex compiles"),
            group: 1,
        };

        assert_eq!(
            extract_response_value(&response, &extractor),
            Err("response body was truncated")
        );
    }

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
