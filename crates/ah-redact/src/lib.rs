//! Secret redaction for AIHelper.
//!
//! Every sink that can persist or transmit user input - the event log, the CLI
//! argv logger, the MCP adapter - has to strip credentials first. This crate is
//! the single owner of that logic so a heuristic added for one sink protects all
//! of them, and so the security-critical parts have one place to be reviewed.
//!
//! Two kinds of function live here:
//!
//! - **Rewriters** (`sanitize_*`) produce a safe copy of a value for a sink that
//!   records it.
//! - **Detectors** (`*_contains_*`, `is_*`) answer whether a value carries a
//!   credential, for a caller that must reject it rather than record it.

use std::{ffi::OsStr, path::Path};

use serde_json::{Map, Value, json};

/// Replacement text for any redacted value. Shared so every sink renders
/// redaction identically.
pub const REDACTED: &str = "[REDACTED]";
/// Marker appended to a value shortened by [`truncate_with_marker`].
pub const TRUNCATED: &str = "...[truncated]";
/// Maximum retained length of a single string value.
pub const MAX_STRING_BYTES: usize = 4 * 1024;
/// Maximum retained number of entries in an array or object.
pub const MAX_COLLECTION_ENTRIES: usize = 100;
/// Maximum retained nesting depth.
pub const MAX_DEPTH: usize = 8;

pub fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(_) => value,
        _ => Value::Object(Map::new()),
    }
}

pub fn sanitize_system_context(context: Value, unredacted: bool) -> Value {
    let Value::Object(mut context) = context else {
        return Value::Object(Map::new());
    };
    let argv = context.remove("argv").and_then(|value| match value {
        Value::Array(values) => Some(
            values
                .into_iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect::<Vec<_>>(),
        ),
        _ => None,
    });
    let mut context = ensure_object(sanitize_value(Value::Object(context), unredacted, 0));
    if let (Some(argv), Some(context)) = (argv, context.as_object_mut()) {
        context.insert("argv".to_owned(), sanitize_cli_argv(argv, unredacted));
    }
    context
}

pub fn sanitize_cli_argv(argv: Vec<String>, unredacted: bool) -> Value {
    let mut sanitized = Vec::with_capacity(argv.len().min(MAX_COLLECTION_ENTRIES));
    let mut redact_next = false;
    let mut always_redact_next = false;
    let truncated = argv.len() > MAX_COLLECTION_ENTRIES;
    let limit = if truncated {
        MAX_COLLECTION_ENTRIES - 1
    } else {
        MAX_COLLECTION_ENTRIES
    };

    for argument in argv.into_iter().take(limit) {
        if redact_next && (!unredacted || always_redact_next) {
            if flag_name(&argument).is_some_and(is_sensitive_cli_flag) {
                sanitized.push(Value::String(bounded_string(&argument)));
                always_redact_next =
                    is_always_redacted_cli_flag(flag_name(&argument).unwrap_or_default());
                continue;
            }
            sanitized.push(Value::String(REDACTED.to_owned()));
            redact_next = false;
            always_redact_next = false;
            continue;
        }

        let bounded = bounded_string(&argument);
        if let Some((prefix, name, value)) = split_flag_assignment(&bounded) {
            if is_sensitive_cli_flag(name) && (!unredacted || is_always_redacted_cli_flag(name)) {
                sanitized.push(Value::String(bounded_string(&format!(
                    "{prefix}{name}={REDACTED}"
                ))));
            } else {
                sanitized.push(Value::String(bounded_string(&format!(
                    "{prefix}{name}={}",
                    sanitize_string(value, unredacted)
                ))));
            }
            continue;
        }

        if let Some(name) = flag_name(&bounded)
            && is_sensitive_cli_flag(name)
            && (!unredacted || is_always_redacted_cli_flag(name))
        {
            redact_next = true;
            always_redact_next = is_always_redacted_cli_flag(name);
            sanitized.push(Value::String(bounded));
            continue;
        }

        sanitized.push(Value::String(sanitize_string(&bounded, unredacted)));
    }

    if truncated {
        sanitized.push(json!({"_truncated": true}));
    }
    Value::Array(sanitized)
}

fn split_flag_assignment(argument: &str) -> Option<(&str, &str, &str)> {
    let prefix_len = argument
        .chars()
        .take_while(|character| *character == '-')
        .count();
    if prefix_len == 0 || prefix_len == argument.len() {
        return None;
    }
    let (prefix, body) = argument.split_at(prefix_len);
    let (name, value) = body.split_once('=')?;
    (!name.is_empty()).then_some((prefix, name, value))
}

fn flag_name(argument: &str) -> Option<&str> {
    let name = argument.trim_start_matches('-');
    (name.len() < argument.len() && !name.is_empty() && !name.contains('=')).then_some(name)
}

pub fn sanitize_value(value: Value, unredacted: bool, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return json!({"_truncated": true});
    }
    match value {
        Value::String(value) => Value::String(sanitize_string(&value, unredacted)),
        Value::Array(values) => {
            let truncated = values.len() > MAX_COLLECTION_ENTRIES;
            let limit = if truncated {
                MAX_COLLECTION_ENTRIES - 1
            } else {
                MAX_COLLECTION_ENTRIES
            };
            let mut bounded = values
                .into_iter()
                .take(limit)
                .map(|value| sanitize_value(value, unredacted, depth + 1))
                .collect::<Vec<_>>();
            if truncated {
                bounded.push(json!({"_truncated": true}));
            }
            Value::Array(bounded)
        }
        Value::Object(values) => {
            let truncated = values.len() > MAX_COLLECTION_ENTRIES;
            let limit = if truncated {
                MAX_COLLECTION_ENTRIES - 1
            } else {
                MAX_COLLECTION_ENTRIES
            };
            let mut bounded = Map::new();
            for (key, value) in values.into_iter().take(limit) {
                let sensitive = is_sensitive_name(&key) && !unredacted;
                bounded.insert(
                    bounded_string(&key),
                    if sensitive {
                        Value::String(REDACTED.to_owned())
                    } else {
                        sanitize_value(value, unredacted, depth + 1)
                    },
                );
            }
            if truncated {
                bounded.insert("_truncated".to_owned(), Value::Bool(true));
            }
            Value::Object(bounded)
        }
        other => other,
    }
}

pub fn sanitize_string(value: &str, unredacted: bool) -> String {
    if unredacted {
        return bounded_string(value);
    }
    if let Some(json) = redact_json_text(value) {
        return bounded_string(&json);
    }
    if let Some(command) = redact_embedded_curl(value) {
        return bounded_string(&command);
    }

    let bounded = bounded_string(value);
    let mut sanitized = redact_bare_userinfo(&redact_urls_in_text(&bounded));
    if let Some((name, separator, header_value)) = split_header_like(&sanitized)
        && is_sensitive_name(name.trim())
    {
        if header_value.trim() == REDACTED {
            return bounded_string(&sanitized);
        }
        return bounded_string(&format!("{name}{separator} {REDACTED}"));
    }

    let trimmed = sanitized.trim_start();
    for prefix in ["bearer", "basic"] {
        if trimmed.len() > prefix.len()
            && trimmed
                .get(..prefix.len())
                .is_some_and(|value| value.eq_ignore_ascii_case(prefix))
            && trimmed.as_bytes()[prefix.len()].is_ascii_whitespace()
        {
            let leading_len = sanitized.len() - trimmed.len();
            return bounded_string(&format!(
                "{}{} {REDACTED}",
                &sanitized[..leading_len],
                trimmed.get(..prefix.len()).unwrap_or_default()
            ));
        }
    }

    while let Some(redacted) = redact_embedded_assignment(&sanitized) {
        if redacted == sanitized {
            break;
        }
        sanitized = redacted;
    }
    bounded_string(&sanitized)
}

fn redact_embedded_assignment(value: &str) -> Option<String> {
    for (separator_index, separator) in value.match_indices(['=', ':']) {
        let key_end = value[..separator_index]
            .trim_end()
            .trim_end_matches(['\'', '"'])
            .len();
        let key_start = value[..key_end]
            .char_indices()
            .rev()
            .find(|(_, character)| {
                !character.is_alphanumeric() && !matches!(character, '_' | '-' | '.')
            })
            .map_or(0, |(index, character)| index + character.len_utf8());
        let key = &value[key_start..key_end];
        if !is_sensitive_name(key) {
            continue;
        }

        let mut value_start = separator_index + separator.len();
        while value[value_start..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            value_start += value[value_start..].chars().next()?.len_utf8();
        }
        let consume_remainder = matches!(
            name_tokens(key).as_slice(),
            [token] if matches!(token.as_str(), "authorization" | "cookie" | "bearer" | "basic")
        );
        let (content_start, value_end) = if consume_remainder {
            (value_start, value.len())
        } else if let Some(quote @ ('\'' | '"')) = value[value_start..].chars().next() {
            let content_start = value_start + quote.len_utf8();
            let value_end = find_unescaped_quote(&value[content_start..], quote)
                .map_or(value.len(), |index| content_start + index);
            (content_start, value_end)
        } else {
            let value_end = value[value_start..]
                .char_indices()
                .find(|(_, character)| {
                    character.is_whitespace() || matches!(character, '&' | ',' | ';')
                })
                .map_or(value.len(), |(index, _)| value_start + index);
            (value_start, value_end)
        };
        if &value[content_start..value_end] == REDACTED {
            continue;
        }
        let mut redacted = String::with_capacity(value.len());
        redacted.push_str(&value[..content_start]);
        redacted.push_str(REDACTED);
        redacted.push_str(&value[value_end..]);
        return Some(bounded_string(&redacted));
    }
    None
}

fn redact_json_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let parsed = serde_json::from_str::<Value>(trimmed).ok()?;
    serde_json::to_string(&sanitize_value(parsed, false, 0)).ok()
}

fn redact_embedded_curl(value: &str) -> Option<String> {
    if !looks_like_curl(value) || value.trim_start().split_ascii_whitespace().nth(1).is_none() {
        return None;
    }
    let redacted = redact_curl_user_options(value);
    let Ok(tokens) = shell_words::split(&redacted) else {
        return Some(redacted);
    };
    let sanitized = sanitize_cli_argv(tokens, false);
    let values = sanitized.as_array()?;
    Some(
        values
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn looks_like_curl(value: &str) -> bool {
    let command = value
        .trim_start()
        .split_ascii_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['\'', '"']);
    Path::new(command)
        .file_stem()
        .and_then(OsStr::to_str)
        .is_some_and(|command| command.eq_ignore_ascii_case("curl"))
}

fn redact_curl_user_options(value: &str) -> String {
    let mut result = value.to_owned();
    let mut index = 0;
    while index < result.len() {
        let bytes = result.as_bytes();
        let boundary = index == 0
            || bytes[index - 1].is_ascii_whitespace()
            || matches!(bytes[index - 1], b'\'' | b'"');
        let option_len = if boundary && result[index..].starts_with("--user") {
            6
        } else if boundary && result[index..].starts_with("-u") {
            2
        } else {
            index += 1;
            continue;
        };
        let option_end = index + option_len;
        if option_end > result.len() {
            break;
        }
        let next = result[option_end..].chars().next();
        if option_len == 6
            && next.is_some_and(|character| character != '=' && !character.is_whitespace())
        {
            index = option_end;
            continue;
        }
        let mut value_start = option_end;
        if result[value_start..].starts_with('=') {
            value_start += 1;
        } else {
            while result[value_start..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
            {
                value_start += result[value_start..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8);
            }
        }
        if value_start >= result.len() {
            break;
        }
        let (content_start, value_end) =
            if let Some(quote @ ('\'' | '"')) = result[value_start..].chars().next() {
                let content_start = value_start + quote.len_utf8();
                let value_end = find_unescaped_quote(&result[content_start..], quote)
                    .map_or(result.len(), |offset| content_start + offset);
                (content_start, value_end)
            } else {
                let value_end = result[value_start..]
                    .char_indices()
                    .find(|(_, character)| character.is_whitespace())
                    .map_or(result.len(), |(offset, _)| value_start + offset);
                (value_start, value_end)
            };
        if &result[content_start..value_end] != REDACTED {
            result.replace_range(content_start..value_end, REDACTED);
        }
        index = content_start + REDACTED.len();
    }
    result
}

fn find_unescaped_quote(value: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
        } else if character == quote {
            return Some(index);
        }
    }
    None
}

fn redact_urls_in_text(value: &str) -> String {
    let mut result = value.to_owned();
    let mut search_start = 0;
    while let Some(relative_scheme) = result[search_start..].find("://") {
        let scheme_end = search_start + relative_scheme;
        let bytes = result.as_bytes();
        let mut url_start = scheme_end;
        while url_start > 0
            && (bytes[url_start - 1].is_ascii_alphanumeric()
                || matches!(bytes[url_start - 1], b'+' | b'-' | b'.'))
        {
            url_start -= 1;
        }
        if url_start == scheme_end {
            search_start = scheme_end + 3;
            continue;
        }
        let mut url_end = scheme_end + 3;
        while url_end < result.len()
            && !result.as_bytes()[url_end].is_ascii_whitespace()
            && !matches!(result.as_bytes()[url_end], b'\'' | b'"' | b'<' | b'>')
        {
            url_end += 1;
        }
        let Some(redacted) = redact_url(&result[url_start..url_end]) else {
            search_start = url_end;
            continue;
        };
        result.replace_range(url_start..url_end, &redacted);
        search_start = url_start + redacted.len();
    }
    result
}

fn redact_bare_userinfo(value: &str) -> String {
    let mut result = value.to_owned();
    let mut search_start = 0;
    while let Some(relative_at) = result[search_start..].find('@') {
        let at = search_start + relative_at;
        let candidate_start = result[..at]
            .char_indices()
            .rev()
            .find(|(_, character)| {
                character.is_whitespace() || matches!(character, '=' | ',' | ';' | '(')
            })
            .map_or(0, |(index, character)| index + character.len_utf8());
        let candidate = &result[candidate_start..at];
        let Some(colon) = candidate.find(':') else {
            search_start = at + 1;
            continue;
        };
        let username = &candidate[..colon];
        let password = &candidate[colon + 1..];
        if username.is_empty()
            || password.is_empty()
            || password == REDACTED
            || password.contains('/')
            || password.contains('\\')
            || result[at + 1..]
                .chars()
                .next()
                .is_none_or(|character| character.is_whitespace())
        {
            search_start = at + 1;
            continue;
        }
        let password_start = candidate_start + colon + 1;
        result.replace_range(password_start..at, REDACTED);
        search_start = password_start + REDACTED.len() + 1;
    }
    result
}

fn split_header_like(value: &str) -> Option<(&str, char, &str)> {
    let colon = value.find(':');
    let equals = value.find('=');
    let index = match (colon, equals) {
        (Some(colon), Some(equals)) => colon.min(equals),
        (Some(colon), None) => colon,
        (None, Some(equals)) => equals,
        (None, None) => return None,
    };
    let separator = value[index..].chars().next()?;
    Some((
        &value[..index],
        separator,
        &value[index + separator.len_utf8()..],
    ))
}

fn redact_url(value: &str) -> Option<String> {
    let scheme_end = value.find("://")?;
    let authority_start = scheme_end + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map(|index| authority_start + index)
        .unwrap_or(value.len());
    let mut result = String::with_capacity(value.len());
    result.push_str(&value[..authority_start]);
    let authority = &value[authority_start..authority_end];
    if let Some(at) = authority.rfind('@') {
        result.push_str(REDACTED);
        result.push('@');
        result.push_str(&authority[at + 1..]);
    } else {
        result.push_str(authority);
    }

    let remainder = &value[authority_end..];
    let Some(query_start) = remainder.find('?') else {
        result.push_str(remainder);
        return Some(bounded_string(&result));
    };
    result.push_str(&remainder[..=query_start]);
    let query_and_fragment = &remainder[query_start + 1..];
    let (query, fragment) = query_and_fragment
        .split_once('#')
        .map_or((query_and_fragment, None), |(query, fragment)| {
            (query, Some(fragment))
        });
    for (index, pair) in query.split('&').enumerate() {
        if index > 0 {
            result.push('&');
        }
        if let Some((name, _value)) = pair.split_once('=')
            && is_sensitive_name(&percent_decode_name(name))
        {
            result.push_str(name);
            result.push('=');
            result.push_str(REDACTED);
        } else {
            result.push_str(pair);
        }
    }
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    Some(bounded_string(&result))
}

fn percent_decode_name(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub fn is_sensitive_name(name: &str) -> bool {
    let tokens = name_tokens(name);
    if tokens.is_empty() {
        return false;
    }
    if tokens.len() == 1 && tokens[0] == "basic" {
        return true;
    }
    if tokens.iter().any(|token| {
        matches!(
            token.as_str(),
            "password"
                | "passwd"
                | "token"
                | "secret"
                | "authorization"
                | "cookie"
                | "credential"
                | "bearer"
        )
    }) {
        return true;
    }
    const COMPOUNDS: &[&[&str]] = &[
        &["api", "key"],
        &["access", "key"],
        &["private", "key"],
        &["client", "secret"],
        &["access", "token"],
        &["refresh", "token"],
    ];
    COMPOUNDS.iter().any(|compound| {
        tokens.windows(compound.len()).any(|window| {
            window
                .iter()
                .map(String::as_str)
                .eq(compound.iter().copied())
        })
    })
}

pub fn is_sensitive_cli_flag(name: &str) -> bool {
    is_sensitive_name(name) || matches!(name.to_ascii_lowercase().as_str(), "u" | "user")
}

fn is_always_redacted_cli_flag(name: &str) -> bool {
    name.eq_ignore_ascii_case("credential")
}

fn name_tokens(name: &str) -> Vec<String> {
    let characters = name.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut token = String::new();
    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_alphanumeric() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
            continue;
        }
        let previous = index.checked_sub(1).and_then(|index| characters.get(index));
        let next = characters.get(index + 1);
        let boundary = !token.is_empty()
            && character.is_uppercase()
            && (previous.is_some_and(|previous| previous.is_lowercase() || previous.is_numeric())
                || (previous.is_some_and(|previous| previous.is_uppercase())
                    && next.is_some_and(|next| next.is_lowercase())));
        if boundary {
            tokens.push(std::mem::take(&mut token));
        }
        token.extend(character.to_lowercase());
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

pub fn bounded_string(value: &str) -> String {
    truncate_with_marker(value, MAX_STRING_BYTES)
}

pub fn truncate_with_marker(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let content_limit = max_bytes.saturating_sub(TRUNCATED.len());
    let mut end = content_limit.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = String::with_capacity(max_bytes);
    truncated.push_str(&value[..end]);
    truncated.push_str(TRUNCATED);
    truncated
}

// --- Detectors -------------------------------------------------------------
//
// A caller that must *reject* a request carrying a plaintext credential asks
// these predicates instead of rewriting the value. They were moved here from the
// MCP adapter so a sink and a gate cannot disagree about what counts as a
// credential.

/// True when `value` is an `Authorization:` header line, case-insensitively.
pub fn is_authorization_header(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("authorization"))
}

pub fn curl_contains_auth(value: &str) -> bool {
    let tokens = match shell_words::split(value) {
        Ok(tokens) => tokens,
        Err(_) => {
            let normalized = value.to_ascii_lowercase();
            return normalized.contains("--user")
                || normalized.contains("authorization:")
                || normalized.split_whitespace().any(|token| token == "-u")
                || value.split_whitespace().any(url_contains_userinfo);
        }
    };
    let mut tokens = tokens.iter();
    while let Some(token) = tokens.next() {
        if matches!(token.as_str(), "-u" | "--user")
            || token.starts_with("--user=")
            || token
                .strip_prefix("-u")
                .is_some_and(|value| !value.is_empty())
            || url_contains_userinfo(token)
        {
            return true;
        }
        if matches!(token.as_str(), "-H" | "--header") {
            if tokens
                .next()
                .is_some_and(|value| is_authorization_header(value))
            {
                return true;
            }
        } else if token
            .strip_prefix("--header=")
            .is_some_and(is_authorization_header)
            || token
                .strip_prefix("-H")
                .is_some_and(|value| !value.is_empty() && is_authorization_header(value))
        {
            return true;
        }
    }
    false
}

pub fn url_contains_userinfo(value: &str) -> bool {
    let Some((_, remainder)) = value.split_once("://") else {
        return false;
    };
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    authority
        .rsplit_once('@')
        .is_some_and(|(userinfo, host)| !userinfo.is_empty() && !host.is_empty())
}
