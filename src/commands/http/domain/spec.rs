//! `ah http assert`: running a spec file's cases in order, each able to use
//! what earlier ones captured.
//!
//! | Module        | Owns                                                  |
//! |---------------|-------------------------------------------------------|
//! | `format`      | the spec file as written on disk                      |
//! | `interpolate` | substituting captured values into a later case        |
//! | `extract`     | pulling those values out of a response                |
//! | `junit`       | the XML a CI run consumes                             |

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

mod extract;
mod format;
mod interpolate;
mod junit;
pub(super) use extract::{Extractor, extract_response_values, parse_extractors};
use format::{OneOrManyStrings, SpecExpect, SpecJsonCheck, SpecRequest, SpecStatusValue};
pub(super) use format::{SpecCase, SpecDefaults, SpecExtractRule, read_spec_file};
pub(super) use interpolate::interpolate_string;
use interpolate::{interpolate_json_value, resolve_case_url};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use extract::extract_response_value;
pub(crate) use junit::render_assert_junit;

pub(super) struct PreparedSpecCase {
    case_name: String,
    request: RequestConfig,
    expectations: RequestExpectations,
    extractors: BTreeMap<String, Extractor>,
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
