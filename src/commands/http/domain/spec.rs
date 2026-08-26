//! The spec-file runner: the on-disk case format, variable interpolation,
//! response extraction, and the JUnit reporter.
//!
//! Split out of `domain.rs` because it is a test framework in its own right.

use std::{collections::BTreeMap, path::Path, time::Duration};

use serde::Deserialize;
use serde_json::Value;

use ah_runtime::core;
use regex::Regex;

use super::{
    AssertReportFormat, DEFAULT_MAX_RESPONSE_BYTES, DEFAULT_TIMEOUT_SECS, HttpAssertCaseOutput,
    HttpAssertOutput, HttpAssertSummary, RequestBody, RequestConfig, ResponseSnapshot,
    assert::{
        JsonExpectation, JsonExpectationOperator, RequestExpectations, evaluate_assertions,
        parse_status_expectation,
    },
    duration_millis,
    jsonpath::{parse_json_path_tokens, resolve_json_path},
    normalize_method, parse_auth, parse_key_value_pair, parse_payload, resolve_spec_relative_path,
    send_with_retry,
};
use crate::{
    commands::http::{AssertArgs, AssertReportArg, RequestOptionsArgs},
    error::AppError,
    output::OutputMode,
};

#[derive(Debug, Deserialize)]
pub(super) struct HttpSpec {
    version: u32,
    #[serde(default)]
    defaults: SpecDefaults,
    #[serde(default)]
    vars: BTreeMap<String, String>,
    #[serde(default)]
    cases: Vec<SpecCase>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct SpecDefaults {
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
pub(super) struct SpecCase {
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
pub(super) struct SpecExtractRule {
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

pub(super) struct PreparedSpecCase {
    case_name: String,
    request: RequestConfig,
    expectations: RequestExpectations,
    extractors: BTreeMap<String, Extractor>,
}

pub(super) enum Extractor {
    Json { path: String },
    Header { name: String },
    Text { regex: Regex, group: usize },
}

pub(super) fn build_case_request(
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

pub(super) fn parse_extractors(
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

pub(super) fn extract_response_values(
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

pub(super) fn extract_response_value(
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

pub(super) fn read_spec_file(path: &Path) -> Result<HttpSpec, AppError> {
    let raw = crate::commands::http::io::read_to_string(path)?;
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

pub(super) fn interpolate_string(
    input: &str,
    vars: &BTreeMap<String, String>,
) -> Result<String, AppError> {
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
        spec_path: core::forward_slashes(&spec_path),
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
