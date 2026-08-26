//! Substituting captured values into a later case's URL, headers and body.

use super::*;

pub(super) fn resolve_case_url(
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

pub(super) fn join_base_and_path(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    }
}

pub(crate) fn interpolate_string(
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

pub(super) fn interpolate_json_value(
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
