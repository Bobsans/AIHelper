//! The event log: what this process did, in a form that can be read after the
//! fact.
//!
//! Every record goes through `ah-redact` before it reaches the disk, because a
//! log is the one artefact that outlives the invocation - a credential written
//! here is a credential leaked for as long as the file exists.

use std::{
    env,
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use ah_mcp::{EventSink, McpCommandEvent, McpCommandStatus};
use ah_redact::{
    REDACTED, ensure_object, sanitize_cli_argv, sanitize_string, sanitize_system_context,
    sanitize_value,
};
use ah_runtime::{
    InvocationOutcome,
    executor::{ExecutionTelemetry, ExecutionTimeoutPhase},
};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use fs2::FileExt;
use serde_json::{Map, Value, json};

use ah_error::AppError;

mod record;
mod rotation;

use record::{RecordKind, bounded_line};
use rotation::{acquire_lock, cleanup_old_logs, log_filename};

pub const SCHEMA_VERSION: u64 = 1;

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
pub enum SystemEventSeverity {
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
pub struct EventDiagnostic {
    pub domain: Option<String>,
    pub operation: Option<String>,
    pub code: String,
    pub message: String,
    pub cause: Option<String>,
    pub exit_code_hint: i32,
    pub retryable: Option<bool>,
}

impl EventDiagnostic {
    pub fn new(code: impl Into<String>, message: impl Into<String>, exit_code_hint: i32) -> Self {
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

    pub fn from_app_error(error: &AppError) -> Self {
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

    pub fn with_identity(mut self, domain: Option<String>, operation: Option<String>) -> Self {
        self.domain = domain;
        self.operation = operation;
        self
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
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

pub struct EventLogger {
    log_dir: PathBuf,
    unredacted: bool,
    clock: Arc<dyn Clock>,
    last_cleanup_date: Mutex<Option<NaiveDate>>,
}

impl EventLogger {
    pub fn new() -> Option<Self> {
        let log_dir = ah_config::resolve_log_dir()?;
        fs::create_dir_all(&log_dir).ok()?;
        Some(Self {
            log_dir,
            unredacted: env::var_os("AH_LOG_UNREDACTED").as_deref() == Some(OsStr::new("1")),
            clock: Arc::new(SystemClock),
            last_cleanup_date: Mutex::new(None),
        })
    }

    pub fn record_cli_command(
        &self,
        command: &str,
        argv: Vec<String>,
        duration: Duration,
        error: Option<&AppError>,
    ) {
        self.record_cli_command_with_outcome(command, argv, duration, None, error);
    }

    pub fn record_cli_command_with_outcome(
        &self,
        command: &str,
        argv: Vec<String>,
        duration: Duration,
        outcome: Option<&InvocationOutcome>,
        error: Option<&AppError>,
    ) {
        let credentialed = argv
            .iter()
            .any(|argument| argument == "--credential" || argument.starts_with("--credential="));
        let diagnostic = error.map(|error| {
            let mut diagnostic = EventDiagnostic::from_app_error(error);
            if credentialed {
                diagnostic.message = "credentialed invocation failed".to_owned();
                diagnostic.cause = Some(REDACTED.to_owned());
            }
            diagnostic
        });
        let status = if diagnostic.is_some() {
            "error"
        } else {
            "success"
        };
        let parameters = sanitize_cli_argv(argv, self.unredacted && !credentialed);
        let mut record = self.command_record(
            "cli",
            command,
            Value::Object(Map::from_iter([("argv".to_owned(), parameters)])),
            status,
            duration_ms(duration),
            diagnostic,
        );
        if let Some(outcome) = outcome {
            record["outcome"] = invocation_outcome_value(*outcome);
        }
        self.write_best_effort(&mut record, RecordKind::Command);
    }

    pub fn record_system_event(
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

    fn record_mcp_command(&self, event: McpCommandEvent, telemetry: Option<ExecutionTelemetry>) {
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
        if let Some(job_id) = event.job_id {
            record["job_id"] = Value::String(sanitize_string(&job_id, self.unredacted));
        }
        if let Some(outcome) = event.outcome {
            record["outcome"] = invocation_outcome_value(outcome);
        }
        if let Some(telemetry) = telemetry {
            record["queue_wait_ms"] = json!(telemetry.queue_wait_ms);
            record["execution_ms"] = json!(telemetry.execution_ms);
            if let Some(timeout_phase) = telemetry.timeout_phase {
                record["timeout_phase"] = Value::String(
                    match timeout_phase {
                        ExecutionTimeoutPhase::Queue => "queue",
                        ExecutionTimeoutPhase::Execution => "execution",
                    }
                    .to_owned(),
                );
            }
        }
        self.write_best_effort(&mut record, RecordKind::Command);
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

fn invocation_outcome_value(outcome: InvocationOutcome) -> Value {
    match outcome {
        InvocationOutcome::RunCheck(outcome) => json!({
            "success": outcome.success,
            "timed_out": outcome.timed_out,
            "exit_code": outcome.exit_code,
        }),
    }
}

impl EventSink for EventLogger {
    fn record_command(&self, event: McpCommandEvent) {
        self.record_mcp_command(event, None);
    }

    fn record_command_with_telemetry(
        &self,
        event: McpCommandEvent,
        telemetry: Option<ExecutionTelemetry>,
    ) {
        self.record_mcp_command(event, telemetry);
    }
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

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc, time::Duration};

    use ah_mcp::{EventSink, McpCommandEvent, McpCommandStatus};
    use ah_plugin_api::CommandError;
    use ah_redact::{
        MAX_STRING_BYTES, REDACTED, is_sensitive_name, sanitize_cli_argv, sanitize_value,
    };
    use ah_runtime::executor::{ExecutionTelemetry, ExecutionTimeoutPhase};
    use chrono::{DateTime, NaiveDate, TimeZone, Utc};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        Clock, EventDiagnostic, EventLogger, SystemEventSeverity, log_filename,
        record::MAX_LINE_BYTES,
    };
    use ah_error::AppError;

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
    fn credential_ids_stay_redacted_in_unredacted_mode() {
        assert_eq!(
            sanitize_cli_argv(
                vec![
                    "--credential".to_owned(),
                    "basic=private-id".to_owned(),
                    "--credential=database=other-id".to_owned(),
                ],
                true,
            ),
            json!(["--credential", "[REDACTED]", "--credential=[REDACTED]"])
        );
    }

    #[test]
    fn credentialed_invocation_redacts_argv_and_error_secrets_in_unredacted_mode() {
        let temp = TempDir::new().unwrap();
        let error = AppError::external("SECRET_NOT_FOUND", "private-id unexpected-secret-sentinel");
        logger(&temp, true).record_cli_command(
            "http.get",
            vec![
                "http".to_owned(),
                "get".to_owned(),
                "--credential".to_owned(),
                "basic=private-id".to_owned(),
                "--basic".to_owned(),
                "legacy:unexpected-secret-sentinel".to_owned(),
            ],
            Duration::ZERO,
            Some(&error),
        );

        let serialized = serde_json::to_string(&records(&temp)).unwrap();
        assert!(!serialized.contains("private-id"));
        assert!(!serialized.contains("unexpected-secret-sentinel"));
        assert!(serialized.contains("credentialed invocation failed"));
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
        EventSink::record_command_with_telemetry(
            &logger,
            McpCommandEvent {
                command: "test.large".to_owned(),
                tool: "ah.test.large".to_owned(),
                request_id: "mcp:test".to_owned(),
                job_id: None,
                parameters,
                status: McpCommandStatus::Success,
                duration_ms: 1,
                diagnostic: None,
                outcome: None,
            },
            Some(ExecutionTelemetry {
                queue_wait_ms: 9,
                execution_ms: 0,
                timeout_phase: Some(ExecutionTimeoutPhase::Queue),
            }),
        );
        let record = &records(&temp)[0];
        assert_eq!(record["record_truncated"], true);
        assert_eq!(record["parameters"]["_truncated"], true);
        assert_eq!(record["queue_wait_ms"], 9);
        assert_eq!(record["execution_ms"], 0);
        assert_eq!(record["timeout_phase"], "queue");
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
        EventSink::record_command_with_telemetry(
            &logger,
            McpCommandEvent {
                command: "search.text".to_owned(),
                tool: "ah.search.text".to_owned(),
                request_id: "mcp:n:7:e:1".to_owned(),
                job_id: Some("job-test".to_owned()),
                parameters: json!({"token": "hidden", "query": "needle"}),
                status: McpCommandStatus::Success,
                duration_ms: 12,
                diagnostic: None,
                outcome: None,
            },
            Some(ExecutionTelemetry {
                queue_wait_ms: 2,
                execution_ms: 8,
                timeout_phase: None,
            }),
        );
        EventSink::record_command(
            &logger,
            McpCommandEvent {
                command: "search.text".to_owned(),
                tool: "ah.search.text".to_owned(),
                request_id: "mcp:n:8:e:2".to_owned(),
                job_id: None,
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
                outcome: None,
            },
        );

        let records = records(&temp);
        assert_eq!(records[0]["transport"], "mcp");
        assert_eq!(records[0]["tool"], "ah.search.text");
        assert_eq!(records[0]["request_id"], "mcp:n:7:e:1");
        assert_eq!(records[0]["parameters"]["token"], "[REDACTED]");
        assert_eq!(records[0]["queue_wait_ms"], 2);
        assert_eq!(records[0]["execution_ms"], 8);
        assert!(records[0].get("timeout_phase").is_none());
        assert!(records[0].get("diagnostic").is_none());
        assert_eq!(records[1]["status"], "error");
        assert_eq!(records[1]["diagnostic"]["code"], "REGEX_INVALID");
        assert_eq!(records[1]["diagnostic"]["retryable"], false);
        assert_eq!(records[1]["diagnostic"]["cause"], "password= [REDACTED]");
        assert!(records[1].get("queue_wait_ms").is_none());
    }
}
