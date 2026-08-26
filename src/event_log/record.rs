//! Bounding and compacting one log record.
//!
//! A record has to fit a line budget whatever the command did, so an oversized
//! one is compacted and, failing that, reduced to a minimal shape that still
//! identifies what happened.

use serde_json::{Value, json};

use ah_redact::truncate_with_marker;

use super::SCHEMA_VERSION;

pub(crate) const COMPACT_DIAGNOSTIC_BYTES: usize = 1024;

pub(crate) const MAX_LINE_BYTES: usize = 65_536;

#[derive(Clone, Copy)]
pub(crate) enum RecordKind {
    Command,
    System,
}

pub(crate) fn bounded_line(record: &mut Value, kind: RecordKind) -> Option<Vec<u8>> {
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

pub(crate) fn compact_record(record: &mut Value, kind: RecordKind, original_bounded_bytes: u64) {
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

pub(crate) fn minimal_record(
    record: &Value,
    kind: RecordKind,
    original_bounded_bytes: u64,
) -> Value {
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
            for field in [
                "request_id",
                "tool",
                "queue_wait_ms",
                "execution_ms",
                "timeout_phase",
            ] {
                if let Some(value) = record.get(field) {
                    minimal[field] = value.clone();
                }
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

pub(crate) fn value_or(record: &Value, field: &str, default: Value) -> Value {
    record.get(field).cloned().unwrap_or(default)
}

pub(crate) fn minimal_diagnostic(diagnostic: Option<&Value>) -> Value {
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn minimal_record_preserves_mcp_identity_and_telemetry() {
        let record = json!({
            "schema_version": 1,
            "timestamp": "2026-07-20T00:00:00.000Z",
            "event": "command.completed",
            "transport": "mcp",
            "pid": 1,
            "command": "file.stat",
            "parameters": {},
            "status": "error",
            "duration_ms": 201,
            "request_id": "mcp:n:1:e:1",
            "tool": "ah.file.stat",
            "queue_wait_ms": 200,
            "execution_ms": 0,
            "timeout_phase": "queue",
            "diagnostic": {
                "code": "TIMEOUT",
                "message": "timed out",
                "exit_code_hint": 1
            }
        });
        let minimal = minimal_record(&record, RecordKind::Command, 70_000);
        assert_eq!(minimal["request_id"], "mcp:n:1:e:1");
        assert_eq!(minimal["tool"], "ah.file.stat");
        assert_eq!(minimal["queue_wait_ms"], 200);
        assert_eq!(minimal["execution_ms"], 0);
        assert_eq!(minimal["timeout_phase"], "queue");
    }
}
