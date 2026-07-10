use std::{collections::BTreeMap, io::Read, path::Path, time::Duration};

use reqwest::{Method, blocking::Client};
use serde_json::Value;

use crate::error::AppError;

use super::super::domain::{AuthConfig, RequestBody, RequestConfig, ResponseSnapshot};

pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, AppError> {
    let path = path.as_ref();
    std::fs::read_to_string(path).map_err(|source| AppError::file_read(path.to_path_buf(), source))
}

pub(crate) fn send_request(request: &RequestConfig) -> Result<ResponseSnapshot, AppError> {
    let method = Method::from_bytes(request.method.as_bytes()).map_err(|error| {
        AppError::invalid_argument(format!("invalid HTTP method '{}': {error}", request.method))
    })?;

    let client = Client::builder()
        .timeout(Duration::from_secs(request.timeout_secs.max(1)))
        .build()
        .map_err(|error| {
            AppError::external(
                "HTTP_CLIENT_BUILD_FAILED",
                format!("failed to build client: {error}"),
            )
        })?;

    let mut builder = client.request(method, &request.url);

    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if !request.query.is_empty() {
        builder = builder.query(&request.query);
    }
    match &request.auth {
        AuthConfig::None => {}
        AuthConfig::Bearer(token) => {
            builder = builder.bearer_auth(token);
        }
        AuthConfig::Basic { username, password } => {
            builder = builder.basic_auth(username, Some(password));
        }
    }
    match &request.body {
        Some(RequestBody::Json(value)) => {
            builder = builder.json(value);
        }
        Some(RequestBody::Text(value)) => {
            builder = builder.body(value.clone());
        }
        None => {}
    }

    let response = builder.send().map_err(|error| {
        AppError::external(
            "HTTP_REQUEST_FAILED",
            format!("request to '{}' failed: {error}", request.url),
        )
    })?;

    let status = response.status();
    let status_code = status.as_u16();
    let status_text = status.canonical_reason().unwrap_or("unknown").to_owned();

    let mut headers = BTreeMap::new();
    for (name, value) in response.headers() {
        let key = name.as_str().to_ascii_lowercase();
        let value_text = match value.to_str() {
            Ok(text) => text.to_owned(),
            Err(_) => String::from_utf8_lossy(value.as_bytes()).into_owned(),
        };
        headers
            .entry(key)
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(&value_text);
            })
            .or_insert(value_text);
    }

    let mut limited = response.take(request.max_response_bytes.saturating_add(1) as u64);
    let mut body_bytes = Vec::with_capacity(request.max_response_bytes.min(64 * 1024));
    limited.read_to_end(&mut body_bytes).map_err(|error| {
        AppError::external(
            "HTTP_RESPONSE_READ_FAILED",
            format!("failed to read response body: {error}"),
        )
    })?;
    let body_truncated = body_bytes.len() > request.max_response_bytes;
    if body_truncated {
        body_bytes.truncate(request.max_response_bytes);
    }
    let body = String::from_utf8_lossy(&body_bytes).into_owned();
    let body_json = if body_truncated {
        None
    } else {
        serde_json::from_str::<Value>(&body).ok()
    };

    Ok(ResponseSnapshot {
        status_code,
        status_text,
        headers,
        body,
        body_json,
        body_truncated,
    })
}
