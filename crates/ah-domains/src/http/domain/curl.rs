//! A parser for the subset of `curl` command lines the replay command accepts.
//!
//! Split out of `domain.rs` because it consumes a string that often comes
//! straight from a browser's "copy as cURL", which is untrusted input.

use serde_json::Value;

use super::{RequestBody, normalize_method, parse_basic_auth, parse_header, parse_timeout_secs};
use ah_error::AppError;

#[derive(Debug)]
pub(super) struct ParsedCurlReplay {
    pub(super) method: Option<String>,
    pub(super) url: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) timeout_secs: Option<u64>,
    pub(super) bearer: Option<String>,
    pub(super) basic: Option<String>,
    pub(super) body: Option<RequestBody>,
}

pub(super) fn parse_curl_replay(raw: &str) -> Result<ParsedCurlReplay, AppError> {
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
        // An empty positional - `curl ''` - is not a URL. Without this the
        // parser hands one out and the failure surfaces further downstream as
        // "url must not be empty", which does not say which argument was wrong.
        url: url
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::invalid_argument("curl command must include URL"))?,
        headers: normalized_headers,
        timeout_secs,
        bearer,
        basic,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::super::hostile_input::{CURL_ALPHABET, Rng};
    use super::*;

    /// A replayed command line is usually pasted from a browser's "copy as
    /// cURL", which makes it untrusted input with shell quoting in it.
    #[test]
    fn generated_command_lines_never_panic() {
        let mut rng = Rng::new(0x5eed_0002);

        for _ in 0..20_000 {
            let raw = rng.string(CURL_ALPHABET, 48);
            if let Ok(parsed) = parse_curl_replay(&raw) {
                // A parse that succeeds has to have found something to request.
                assert!(!parsed.url.is_empty(), "{raw:?} parsed to an empty url");
            }
        }
    }

    /// Unbalanced quoting and trailing escapes are the two shapes a truncated
    /// paste actually takes.
    #[test]
    fn truncated_pastes_are_rejected_not_survived() {
        for raw in [
            "curl 'https://example.test",
            "curl \"https://example.test",
            "curl https://example.test \\",
            "curl -H",
            "curl",
            "",
        ] {
            let _ = parse_curl_replay(raw);
        }
    }
}
