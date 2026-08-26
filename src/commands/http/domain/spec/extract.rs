//! Pulling values out of a response so a later case can use them.

use super::*;

pub(crate) enum Extractor {
    Json { path: String },
    Header { name: String },
    Text { regex: Regex, group: usize },
}

pub(crate) fn parse_extractors(
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

pub(crate) fn extract_response_values(
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
