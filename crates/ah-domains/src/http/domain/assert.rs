//! The request-expectation DSL: parsing `--expect-*` flags and spec
//! expectations, and evaluating them against a response.
//!
//! Split out of `domain.rs` because it is a separately testable language with
//! its own grammar, and because it consumes strings a caller supplies.

use regex::Regex;
use serde_json::Value;

use super::{AssertionSummary, ResponseSnapshot, jsonpath::resolve_json_path, parse_header};
use ah_error::AppError;

use crate::http::RequestExpectArgs;

#[derive(Debug, Default)]
pub(super) struct RequestExpectations {
    pub(super) status: Option<StatusExpectation>,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body_contains: Vec<String>,
    pub(super) json: Vec<JsonExpectation>,
}

#[derive(Debug, Clone)]
pub(super) enum StatusExpectation {
    Exact(u16),
    Class(u16),
    Range(u16, u16),
}

#[derive(Debug, Clone)]
pub(super) struct JsonExpectation {
    pub(super) path: String,
    pub(super) operator: JsonExpectationOperator,
    pub(super) source: String,
}

#[derive(Debug, Clone)]
pub(super) enum JsonExpectationOperator {
    Eq(Value),
    Contains(Value),
    Exists(bool),
    Match(Regex),
}

pub(super) fn parse_request_expectations(
    args: &RequestExpectArgs,
) -> Result<RequestExpectations, AppError> {
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

pub(super) fn evaluate_assertions(
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

pub(super) fn parse_status_expectation(raw: &str) -> Result<StatusExpectation, AppError> {
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

pub(super) fn parse_json_expectation_expression(raw: &str) -> Result<JsonExpectation, AppError> {
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
}
