use std::{
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use ah_mcp::{EventSink, McpCommandEvent, McpCommandStatus};
use chrono::{DateTime, Days, NaiveDate, SecondsFormat, Utc};
use fs2::FileExt;
use serde_json::{Map, Value, json};

use crate::{config, error::AppError};

const SCHEMA_VERSION: u64 = 1;
const FILE_PREFIX: &str = "aihelper-";
const FILE_SUFFIX: &str = ".jsonl";
const REDACTED: &str = "[REDACTED]";
const TRUNCATED: &str = "...[truncated]";
const MAX_STRING_BYTES: usize = 4 * 1024;
const COMPACT_DIAGNOSTIC_BYTES: usize = 1024;
const MAX_COLLECTION_ENTRIES: usize = 100;
const MAX_DEPTH: usize = 8;
const MAX_LINE_BYTES: usize = 65_536;
const LOCK_TIMEOUT: Duration = Duration::from_millis(50);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(2);

trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SystemEventSeverity {
    Warning,
    Error,
}

impl SystemEventSeverity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct EventDiagnostic {
    pub(crate) domain: Option<String>,
    pub(crate) operation: Option<String>,
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) cause: Option<String>,
    pub(crate) exit_code_hint: i32,
    pub(crate) retryable: Option<bool>,
}

impl EventDiagnostic {
    pub(crate) fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        exit_code_hint: i32,
    ) -> Self {
        Self {
            domain: None,
            operation: None,
            code: code.into(),
            message: message.into(),
            cause: None,
            exit_code_hint,
            retryable: None,
        }
    }

    pub(crate) fn from_app_error(error: &AppError) -> Self {
        let diagnostic = error.diagnostic();
        Self {
            domain: diagnostic.domain,
            operation: diagnostic.operation,
            code: diagnostic.code,
            message: diagnostic.message,
            cause: non_empty(diagnostic.cause),
            exit_code_hint: diagnostic.exit_code_hint,
            retryable: None,
        }
    }

    pub(crate) fn with_identity(
        mut self,
        domain: Option<String>,
        operation: Option<String>,
    ) -> Self {
        self.domain = domain;
        self.operation = operation;
        self
    }

    pub(crate) fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.cause = non_empty(cause.into());
        self
    }

    fn into_value(self, unredacted: bool) -> Value {
        let mut value = Map::new();
        if let Some(domain) = self.domain {
            value.insert("domain".to_owned(), Value::String(domain));
        }
        if let Some(operation) = self.operation {
            value.insert("operation".to_owned(), Value::String(operation));
        }
        value.insert("code".to_owned(), Value::String(self.code));
        value.insert("message".to_owned(), Value::String(self.message));
        if let Some(cause) = self.cause {
            value.insert("cause".to_owned(), Value::String(cause));
        }
        value.insert("exit_code_hint".to_owned(), json!(self.exit_code_hint));
        if let Some(retryable) = self.retryable {
            value.insert("retryable".to_owned(), Value::Bool(retryable));
        }
        sanitize_value(Value::Object(value), unredacted, 0)
    }
}

pub(crate) struct EventLogger {
    log_dir: PathBuf,
    unredacted: bool,
    clock: Arc<dyn Clock>,
    last_cleanup_date: Mutex<Option<NaiveDate>>,
}

impl EventLogger {
    pub(crate) fn new() -> Option<Self> {
        let log_dir = config::resolve_log_dir()?;
        fs::create_dir_all(&log_dir).ok()?;
        Some(Self {
            log_dir,
            unredacted: env::var_os("AH_LOG_UNREDACTED").as_deref() == Some(OsStr::new("1")),
            clock: Arc::new(SystemClock),
            last_cleanup_date: Mutex::new(None),
        })
    }

    pub(crate) fn record_cli_command(
        &self,
        command: &str,
        argv: Vec<String>,
        duration: Duration,
        error: Option<&AppError>,
    ) {
        let diagnostic = error.map(EventDiagnostic::from_app_error);
        let status = if diagnostic.is_some() {
            "error"
        } else {
            "success"
        };
        let parameters = sanitize_cli_argv(argv, self.unredacted);
        let mut record = self.command_record(
            "cli",
            command,
            Value::Object(Map::from_iter([("argv".to_owned(), parameters)])),
            status,
            duration_ms(duration),
            diagnostic,
        );
        self.write_best_effort(&mut record, RecordKind::Command);
    }

    pub(crate) fn record_system_event(
        &self,
        component: &str,
        severity: SystemEventSeverity,
        diagnostic: EventDiagnostic,
        context: Value,
    ) {
        let now = self.clock.now();
        let date = now.date_naive();
        let mut record = json!({
            "schema_version": SCHEMA_VERSION,
            "timestamp": timestamp(&now),
            "event": "system",
            "pid": std::process::id(),
            "component": sanitize_string(component, self.unredacted),
            "severity": severity.as_str(),
            "diagnostic": diagnostic.into_value(self.unredacted),
            "context": sanitize_system_context(context, self.unredacted),
        });
        self.write_best_effort_at(&mut record, RecordKind::System, date);
    }

    fn command_record(
        &self,
        transport: &str,
        command: &str,
        parameters: Value,
        status: &str,
        duration_ms: u64,
        diagnostic: Option<EventDiagnostic>,
    ) -> Value {
        let now = self.clock.now();
        let mut record = json!({
            "schema_version": SCHEMA_VERSION,
            "timestamp": timestamp(&now),
            "event": "command.completed",
            "transport": transport,
            "pid": std::process::id(),
            "command": sanitize_string(command, self.unredacted),
            "parameters": ensure_object(sanitize_value(parameters, self.unredacted, 0)),
            "status": status,
            "duration_ms": duration_ms,
        });
        if status == "error" {
            let diagnostic = diagnostic.unwrap_or_else(|| {
                EventDiagnostic::new(
                    "EVENT_DIAGNOSTIC_MISSING",
                    "command failed without a diagnostic",
                    1,
                )
            });
            record["diagnostic"] = diagnostic.into_value(self.unredacted);
        }
        record
    }

    fn write_best_effort(&self, record: &mut Value, kind: RecordKind) {
        let date = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.date_naive())
            .unwrap_or_else(|| self.clock.now().date_naive());
        self.write_best_effort_at(record, kind, date);
    }

    fn write_best_effort_at(&self, record: &mut Value, kind: RecordKind, date: NaiveDate) {
        let Some(line) = bounded_line(record, kind) else {
            return;
        };
        let _ = self.try_write(&line, date);
    }

    fn try_write(&self, line: &[u8], date: NaiveDate) -> io::Result<()> {
        fs::create_dir_all(&self.log_dir)?;
        self.cleanup_once(date);
        let path = self.log_dir.join(log_filename(date));
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && (metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log destination is not a regular file",
            ));
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "opened log destination is not a regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        acquire_lock(&file)?;
        let result = (|| {
            file.write_all(line)?;
            file.write_all(b"\n")?;
            file.flush()
        })();
        let _ = FileExt::unlock(&file);
        result
    }

    fn cleanup_once(&self, date: NaiveDate) {
        let Ok(mut last_cleanup_date) = self.last_cleanup_date.lock() else {
            return;
        };
        if *last_cleanup_date == Some(date) {
            return;
        }
        *last_cleanup_date = Some(date);
        drop(last_cleanup_date);
        let _ = cleanup_old_logs(&self.log_dir, date);
    }

    #[cfg(test)]
    fn for_test(log_dir: PathBuf, unredacted: bool, clock: Arc<dyn Clock>) -> Self {
        fs::create_dir_all(&log_dir).expect("test log directory should be created");
        Self {
            log_dir,
            unredacted,
            clock,
            last_cleanup_date: Mutex::new(None),
        }
    }
}

impl EventSink for EventLogger {
    fn record_command(&self, event: McpCommandEvent) {
        let status = match event.status {
            McpCommandStatus::Success => "success",
            McpCommandStatus::Error => "error",
        };
        let diagnostic = event.diagnostic.map(|diagnostic| EventDiagnostic {
            domain: diagnostic.domain,
            operation: diagnostic.operation,
            code: diagnostic.code,
            message: diagnostic.message,
            cause: non_empty(diagnostic.cause),
            exit_code_hint: diagnostic.exit_code_hint,
            retryable: Some(diagnostic.retryable),
        });
        let mut record = self.command_record(
            "mcp",
            &event.command,
            event.parameters,
            status,
            event.duration_ms,
            diagnostic,
        );
        record["request_id"] = Value::String(sanitize_string(&event.request_id, self.unredacted));
        record["tool"] = Value::String(sanitize_string(&event.tool, self.unredacted));
        self.write_best_effort(&mut record, RecordKind::Command);
    }
}

#[derive(Clone, Copy)]
enum RecordKind {
    Command,
    System,
}

fn timestamp(now: &DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn ensure_object(value: Value) -> Value {
    match value {
        Value::Object(_) => value,
        _ => Value::Object(Map::new()),
    }
}

fn sanitize_system_context(context: Value, unredacted: bool) -> Value {
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

fn sanitize_cli_argv(argv: Vec<String>, unredacted: bool) -> Value {
    let mut sanitized = Vec::with_capacity(argv.len().min(MAX_COLLECTION_ENTRIES));
    let mut redact_next = false;
    let truncated = argv.len() > MAX_COLLECTION_ENTRIES;
    let limit = if truncated {
        MAX_COLLECTION_ENTRIES - 1
    } else {
        MAX_COLLECTION_ENTRIES
    };

    for argument in argv.into_iter().take(limit) {
        if redact_next && !unredacted {
            if flag_name(&argument).is_some_and(is_sensitive_cli_flag) {
                sanitized.push(Value::String(bounded_string(&argument)));
                continue;
            }
            sanitized.push(Value::String(REDACTED.to_owned()));
            redact_next = false;
            continue;
        }

        let bounded = bounded_string(&argument);
        if let Some((prefix, name, value)) = split_flag_assignment(&bounded) {
            if is_sensitive_cli_flag(name) && !unredacted {
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
            && !unredacted
        {
            redact_next = true;
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

fn sanitize_value(value: Value, unredacted: bool, depth: usize) -> Value {
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

fn sanitize_string(value: &str, unredacted: bool) -> String {
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

fn is_sensitive_name(name: &str) -> bool {
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

fn is_sensitive_cli_flag(name: &str) -> bool {
    is_sensitive_name(name) || matches!(name.to_ascii_lowercase().as_str(), "u" | "user")
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

fn bounded_string(value: &str) -> String {
    truncate_with_marker(value, MAX_STRING_BYTES)
}

fn truncate_with_marker(value: &str, max_bytes: usize) -> String {
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

fn bounded_line(record: &mut Value, kind: RecordKind) -> Option<Vec<u8>> {
    let first = serde_json::to_vec(record).ok()?;
    if first.len() < MAX_LINE_BYTES {
        return Some(first);
    }
    let original_bounded_bytes = first.len().saturating_add(1) as u64;
    compact_record(record, kind, original_bounded_bytes);
    let second = serde_json::to_vec(record).ok()?;
    if second.len() < MAX_LINE_BYTES {
        return Some(second);
    }
    let minimal = minimal_record(record, kind, original_bounded_bytes);
    let line = serde_json::to_vec(&minimal).ok()?;
    (line.len() < MAX_LINE_BYTES).then_some(line)
}

fn compact_record(record: &mut Value, kind: RecordKind, original_bounded_bytes: u64) {
    let payload = match kind {
        RecordKind::Command => "parameters",
        RecordKind::System => "context",
    };
    record[payload] = json!({
        "_truncated": true,
        "original_bounded_bytes": original_bounded_bytes,
    });
    record["record_truncated"] = Value::Bool(true);
    if let Some(diagnostic) = record.get_mut("diagnostic").and_then(Value::as_object_mut) {
        for field in ["message", "cause"] {
            if let Some(value) = diagnostic
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                diagnostic[field] =
                    Value::String(truncate_with_marker(&value, COMPACT_DIAGNOSTIC_BYTES));
            }
        }
    }
}

fn minimal_record(record: &Value, kind: RecordKind, original_bounded_bytes: u64) -> Value {
    let compact_payload = json!({
        "_truncated": true,
        "original_bounded_bytes": original_bounded_bytes,
    });
    match kind {
        RecordKind::Command => {
            let mut minimal = json!({
                "schema_version": value_or(record, "schema_version", json!(SCHEMA_VERSION)),
                "timestamp": value_or(record, "timestamp", Value::String(String::new())),
                "event": "command.completed",
                "transport": value_or(record, "transport", Value::String("cli".to_owned())),
                "pid": value_or(record, "pid", json!(std::process::id())),
                "command": value_or(record, "command", Value::String(String::new())),
                "parameters": compact_payload,
                "status": value_or(record, "status", Value::String("error".to_owned())),
                "duration_ms": value_or(record, "duration_ms", json!(0)),
                "record_truncated": true,
            });
            if minimal["status"] == "error" {
                minimal["diagnostic"] = minimal_diagnostic(record.get("diagnostic"));
            }
            minimal
        }
        RecordKind::System => json!({
            "schema_version": value_or(record, "schema_version", json!(SCHEMA_VERSION)),
            "timestamp": value_or(record, "timestamp", Value::String(String::new())),
            "event": "system",
            "pid": value_or(record, "pid", json!(std::process::id())),
            "component": value_or(record, "component", Value::String("startup".to_owned())),
            "severity": value_or(record, "severity", Value::String("error".to_owned())),
            "diagnostic": minimal_diagnostic(record.get("diagnostic")),
            "context": compact_payload,
            "record_truncated": true,
        }),
    }
}

fn value_or(record: &Value, field: &str, default: Value) -> Value {
    record.get(field).cloned().unwrap_or(default)
}

fn minimal_diagnostic(diagnostic: Option<&Value>) -> Value {
    let code = diagnostic
        .and_then(|value| value.get("code"))
        .cloned()
        .unwrap_or_else(|| Value::String("DIAGNOSTIC_TRUNCATED".to_owned()));
    let exit_code_hint = diagnostic
        .and_then(|value| value.get("exit_code_hint"))
        .cloned()
        .unwrap_or_else(|| json!(1));
    let mut minimal = json!({
        "code": code,
        "message": "diagnostic truncated",
        "exit_code_hint": exit_code_hint,
    });
    for field in ["domain", "operation"] {
        if let Some(value) = diagnostic.and_then(|value| value.get(field)) {
            minimal[field] = value.clone();
        }
    }
    minimal
}

fn acquire_lock(file: &File) -> io::Result<()> {
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(()),
            Err(error) if retryable_lock_error(&error) => {
                if started.elapsed() >= LOCK_TIMEOUT {
                    return Err(error);
                }
                thread::sleep(LOCK_RETRY_DELAY.min(LOCK_TIMEOUT.saturating_sub(started.elapsed())));
            }
            Err(error) => return Err(error),
        }
    }
}

fn retryable_lock_error(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        matches!(
            error.raw_os_error(),
            Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
        )
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn log_filename(date: NaiveDate) -> String {
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", date.format("%Y-%m-%d"))
}

fn cleanup_old_logs(log_dir: &Path, current_date: NaiveDate) -> io::Result<()> {
    let oldest_retained = current_date
        .checked_sub_days(Days::new(9))
        .unwrap_or(NaiveDate::MIN);
    for entry in fs::read_dir(log_dir)? {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let Some(date) = entry.file_name().to_str().and_then(parse_log_filename) else {
            continue;
        };
        if date < oldest_retained {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn parse_log_filename(name: &str) -> Option<NaiveDate> {
    if name.len() != FILE_PREFIX.len() + 10 + FILE_SUFFIX.len()
        || !name.starts_with(FILE_PREFIX)
        || !name.ends_with(FILE_SUFFIX)
    {
        return None;
    }
    NaiveDate::parse_from_str(&name[FILE_PREFIX.len()..FILE_PREFIX.len() + 10], "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc, time::Duration};

    use ah_mcp::{EventSink, McpCommandEvent, McpCommandStatus};
    use ah_plugin_api::CommandError;
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        Clock, EventDiagnostic, EventLogger, MAX_LINE_BYTES, MAX_STRING_BYTES, REDACTED,
        SystemEventSeverity, is_sensitive_name, log_filename, sanitize_cli_argv, sanitize_value,
    };

    struct FixedClock(DateTime<Utc>);

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 19, 14, 25, 31)
            .single()
            .unwrap()
            + chrono::TimeDelta::milliseconds(123)
    }

    fn logger(temp: &TempDir, unredacted: bool) -> EventLogger {
        EventLogger::for_test(
            temp.path().to_path_buf(),
            unredacted,
            Arc::new(FixedClock(fixed_time())),
        )
    }

    fn records(temp: &TempDir) -> Vec<Value> {
        let path = temp.path().join(log_filename(fixed_time().date_naive()));
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn sensitive_name_tokenization_handles_acronyms_and_false_positives() {
        for sensitive in [
            "accessToken",
            "APIKey",
            "x_api_key",
            "client-secret",
            "privateKey",
            "authorization",
            "basic",
        ] {
            assert!(is_sensitive_name(sensitive), "{sensitive}");
        }
        for safe in ["monkey", "tokenizer", "basics", "secretary", "api"] {
            assert!(!is_sensitive_name(safe), "{safe}");
        }
    }

    #[test]
    fn recursively_redacts_keys_headers_urls_and_diagnostics() {
        let value = sanitize_value(
            json!({
                "outer": {"clientSecret": "hidden", "monkey": "visible"},
                "headers": ["Authorization: Bearer abc", "X-Trace: visible"],
                "url": "https://user:pass@example.test/path?apiKey=abc&monkey=banana",
                "message": "request failed: password=secret-value retrying"
            }),
            false,
            0,
        );
        assert_eq!(value["outer"]["clientSecret"], "[REDACTED]");
        assert_eq!(value["outer"]["monkey"], "visible");
        assert_eq!(value["headers"][0], "Authorization: [REDACTED]");
        assert_eq!(value["headers"][1], "X-Trace: visible");
        assert_eq!(
            value["url"],
            "https://[REDACTED]@example.test/path?apiKey=[REDACTED]&monkey=banana"
        );
        assert_eq!(
            value["message"],
            "request failed: password=[REDACTED] retrying"
        );
    }

    #[test]
    fn default_redaction_accepts_multibyte_strings() {
        let value = sanitize_value(Value::String("ééé 😀".to_owned()), false, 0);
        assert_eq!(value, "ééé 😀");
    }

    #[test]
    fn redacts_supported_curl_and_json_credentials() {
        let curl = sanitize_value(
            Value::String("curl -u alice:s3cr3t --user=bob:hidden https://example.test".to_owned()),
            false,
            0,
        );
        let curl = curl.as_str().unwrap();
        assert!(!curl.contains("s3cr3t"));
        assert!(!curl.contains("hidden"));
        assert!(curl.matches(REDACTED).count() >= 2);

        let json = sanitize_value(
            Value::String(
                r#"{"safe":1,"password":"hidden","nested":{"accessToken":"secret"}}"#.to_owned(),
            ),
            false,
            0,
        );
        let parsed: Value = serde_json::from_str(json.as_str().unwrap()).unwrap();
        assert_eq!(parsed["safe"], 1);
        assert_eq!(parsed["password"], REDACTED);
        assert_eq!(parsed["nested"]["accessToken"], REDACTED);
    }

    #[test]
    fn redacts_attached_and_malformed_curl_credentials() {
        for command in [
            "curl -ualice:attached https://example.test",
            "curl --user bob:separate https://example.test",
            "curl -u \"carol:unterminated",
            "curl \"--user=dave:quoted-long",
            "curl '-uerin:quoted-short",
        ] {
            let sanitized = sanitize_value(Value::String(command.to_owned()), false, 0);
            let sanitized = sanitized.as_str().unwrap();
            assert!(sanitized.contains(REDACTED), "{sanitized}");
            for secret in [
                "attached",
                "separate",
                "unterminated",
                "quoted-long",
                "quoted-short",
            ] {
                assert!(!sanitized.contains(secret), "{sanitized}");
            }
        }
    }

    #[test]
    fn redacts_multiple_mixed_secrets_in_one_string() {
        let value = sanitize_value(
            Value::String(
                "endpoint=https://user:pass@example.test?api%5Fkey=one token=two password=three"
                    .to_owned(),
            ),
            false,
            0,
        );
        let value = value.as_str().unwrap();
        assert_eq!(
            value,
            "endpoint=https://[REDACTED]@example.test?api%5Fkey=[REDACTED] token=[REDACTED] password=[REDACTED]"
        );
    }

    #[test]
    fn redacts_quoted_partial_marker_and_bare_userinfo_secrets() {
        let value = sanitize_value(
            Value::String(
                "error=bad token=\"secret value\" password=[REDACTED]real connection=user:pass@host"
                    .to_owned(),
            ),
            false,
            0,
        );
        assert_eq!(
            value.as_str().unwrap(),
            "error=bad token=\"[REDACTED]\" password=[REDACTED] connection=user:[REDACTED]@host"
        );
    }

    #[test]
    fn redacts_quoted_secrets_with_escaped_quotes() {
        let value = sanitize_value(
            Value::String(
                r#"error=bad token="secret\" remaining-secret" password='single\' remaining'"#
                    .to_owned(),
            ),
            false,
            0,
        );
        assert_eq!(
            value.as_str().unwrap(),
            r#"error=bad token="[REDACTED]" password='[REDACTED]'"#
        );
    }

    #[test]
    fn cli_argv_redacts_adjacent_and_assignment_values() {
        let temp = TempDir::new().unwrap();
        logger(&temp, false).record_cli_command(
            "http.get",
            vec![
                "http".to_owned(),
                "get".to_owned(),
                "--token".to_owned(),
                "adjacent-secret".to_owned(),
                "--APIKey=assigned-secret".to_owned(),
                "--header".to_owned(),
                "Cookie=session-secret".to_owned(),
                "monkey=value".to_owned(),
            ],
            Duration::from_millis(7),
            None,
        );
        let record = &records(&temp)[0];
        assert_eq!(
            record["parameters"]["argv"],
            json!([
                "http",
                "get",
                "--token",
                "[REDACTED]",
                "--APIKey=[REDACTED]",
                "--header",
                "Cookie= [REDACTED]",
                "monkey=value"
            ])
        );
    }

    #[test]
    fn cli_argv_keeps_redaction_across_consecutive_sensitive_flags() {
        let value = sanitize_cli_argv(
            vec![
                "--bearer".to_owned(),
                "--user".to_owned(),
                "alice:secret".to_owned(),
            ],
            false,
        );
        assert_eq!(value, json!(["--bearer", "--user", "[REDACTED]"]));
    }

    #[test]
    fn unredacted_mode_preserves_secrets_but_still_bounds_strings() {
        let temp = TempDir::new().unwrap();
        let secret = format!("password={}", "x".repeat(MAX_STRING_BYTES * 2));
        logger(&temp, true).record_cli_command(
            "test.command",
            vec!["--token".to_owned(), secret],
            Duration::ZERO,
            None,
        );
        let record = &records(&temp)[0];
        assert_eq!(record["parameters"]["argv"][0], "--token");
        let value = record["parameters"]["argv"][1].as_str().unwrap();
        assert!(value.starts_with("password="));
        assert!(value.ends_with("...[truncated]"));
        assert!(value.len() <= MAX_STRING_BYTES);
    }

    #[test]
    fn string_bounds_preserve_valid_utf8() {
        let value = sanitize_value(Value::String("é".repeat(MAX_STRING_BYTES)), true, 0);
        let value = value.as_str().unwrap();
        assert!(value.is_char_boundary(value.len()));
        assert!(value.len() <= MAX_STRING_BYTES);
        assert!(value.ends_with("...[truncated]"));
    }

    #[test]
    fn writes_valid_bounded_jsonl_for_cli_and_system_events() {
        let temp = TempDir::new().unwrap();
        let logger = logger(&temp, false);
        logger.record_cli_command(
            "plugins.list",
            vec!["plugins".to_owned(), "list".to_owned()],
            Duration::from_millis(12),
            None,
        );
        logger.record_system_event(
            "plugin_discovery",
            SystemEventSeverity::Warning,
            EventDiagnostic::new("PLUGIN_SKIPPED", "plugin was skipped", 0)
                .with_cause("authorization=secret")
                .with_identity(Some("plugins".to_owned()), None),
            json!({"path": "missing"}),
        );

        let path = temp.path().join(log_filename(fixed_time().date_naive()));
        let contents = fs::read_to_string(path).unwrap();
        assert!(contents.ends_with('\n'));
        for line in contents.lines() {
            assert!(line.len() < MAX_LINE_BYTES);
            let _: Value = serde_json::from_str(line).unwrap();
        }
        let records = records(&temp);
        assert_eq!(records[0]["schema_version"], 1);
        assert_eq!(records[0]["timestamp"], "2026-07-19T14:25:31.123Z");
        assert_eq!(records[0]["event"], "command.completed");
        assert_eq!(records[0]["transport"], "cli");
        assert_eq!(records[0]["status"], "success");
        assert!(records[0].get("diagnostic").is_none());
        assert_eq!(records[1]["event"], "system");
        assert_eq!(records[1]["severity"], "warning");
        assert_eq!(
            records[1]["diagnostic"]["cause"],
            "authorization= [REDACTED]"
        );
    }

    #[test]
    fn system_context_uses_cli_argv_redaction() {
        let temp = TempDir::new().unwrap();
        logger(&temp, false).record_system_event(
            "cli_parse",
            SystemEventSeverity::Error,
            EventDiagnostic::new("INVALID_ARGUMENT", "invalid arguments", 1),
            json!({"argv": ["http", "get", "--bearer", "hidden-token"]}),
        );
        let record = &records(&temp)[0];
        assert_eq!(
            record["context"]["argv"],
            json!(["http", "get", "--bearer", "[REDACTED]"])
        );
    }

    #[test]
    fn oversized_records_use_compact_fallback_and_remain_bounded() {
        let temp = TempDir::new().unwrap();
        let logger = logger(&temp, true);
        let parameters = Value::Object(
            (0..100)
                .map(|index| (format!("field_{index}"), Value::String("x".repeat(4096))))
                .collect(),
        );
        EventSink::record_command(
            &logger,
            McpCommandEvent {
                command: "test.large".to_owned(),
                tool: "ah.test.large".to_owned(),
                request_id: "mcp:test".to_owned(),
                parameters,
                status: McpCommandStatus::Success,
                duration_ms: 1,
                diagnostic: None,
            },
        );
        let record = &records(&temp)[0];
        assert_eq!(record["record_truncated"], true);
        assert_eq!(record["parameters"]["_truncated"], true);
        let line = serde_json::to_vec(record).unwrap();
        assert!(line.len() < MAX_LINE_BYTES);
    }

    #[test]
    fn cleanup_retains_exact_utc_window_and_ignores_other_files() {
        let temp = TempDir::new().unwrap();
        let current = NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
        for days_ago in 0..=10 {
            let date = current
                .checked_sub_days(chrono::Days::new(days_ago))
                .unwrap();
            fs::write(temp.path().join(log_filename(date)), "old\n").unwrap();
        }
        fs::write(temp.path().join("aihelper-invalid.jsonl"), "keep").unwrap();
        fs::write(temp.path().join("other-2026-01-01.jsonl"), "keep").unwrap();
        let future = current.checked_add_days(chrono::Days::new(1)).unwrap();
        fs::write(temp.path().join(log_filename(future)), "future\n").unwrap();

        let logger = logger(&temp, false);
        logger.record_cli_command("test", vec![], Duration::ZERO, None);

        for days_ago in 0..10 {
            let date = current
                .checked_sub_days(chrono::Days::new(days_ago))
                .unwrap();
            assert!(temp.path().join(log_filename(date)).exists());
        }
        let expired = current.checked_sub_days(chrono::Days::new(10)).unwrap();
        assert!(!temp.path().join(log_filename(expired)).exists());
        assert!(temp.path().join("aihelper-invalid.jsonl").exists());
        assert!(temp.path().join("other-2026-01-01.jsonl").exists());
        assert!(temp.path().join(log_filename(future)).exists());
    }

    #[test]
    fn cleanup_does_not_follow_matching_symlinks() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target.txt");
        fs::write(&target, "keep").unwrap();
        let link = temp.path().join("aihelper-2000-01-01.jsonl");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        logger(&temp, false).record_cli_command("test", vec![], Duration::ZERO, None);
        assert!(link.exists());
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }

    #[test]
    fn writer_rejects_matching_symlink_destination() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target.txt");
        fs::write(&target, "keep").unwrap();
        let link = temp.path().join(log_filename(fixed_time().date_naive()));
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            return;
        }

        logger(&temp, false).record_cli_command("test", vec![], Duration::ZERO, None);
        assert_eq!(fs::read_to_string(target).unwrap(), "keep");
    }

    #[test]
    fn maps_mcp_success_and_error_events() {
        let temp = TempDir::new().unwrap();
        let logger = logger(&temp, false);
        EventSink::record_command(
            &logger,
            McpCommandEvent {
                command: "search.text".to_owned(),
                tool: "ah.search.text".to_owned(),
                request_id: "mcp:n:7:e:1".to_owned(),
                parameters: json!({"token": "hidden", "query": "needle"}),
                status: McpCommandStatus::Success,
                duration_ms: 12,
                diagnostic: None,
            },
        );
        EventSink::record_command(
            &logger,
            McpCommandEvent {
                command: "search.text".to_owned(),
                tool: "ah.search.text".to_owned(),
                request_id: "mcp:n:8:e:2".to_owned(),
                parameters: json!({}),
                status: McpCommandStatus::Error,
                duration_ms: 3,
                diagnostic: Some(CommandError::new(
                    Some("search".to_owned()),
                    Some("search.text".to_owned()),
                    "REGEX_INVALID",
                    "invalid regular expression",
                    "password=hidden",
                    1,
                    false,
                )),
            },
        );

        let records = records(&temp);
        assert_eq!(records[0]["transport"], "mcp");
        assert_eq!(records[0]["tool"], "ah.search.text");
        assert_eq!(records[0]["request_id"], "mcp:n:7:e:1");
        assert_eq!(records[0]["parameters"]["token"], "[REDACTED]");
        assert!(records[0].get("diagnostic").is_none());
        assert_eq!(records[1]["status"], "error");
        assert_eq!(records[1]["diagnostic"]["code"], "REGEX_INVALID");
        assert_eq!(records[1]["diagnostic"]["retryable"], false);
        assert_eq!(records[1]["diagnostic"]["cause"], "password= [REDACTED]");
    }
}
