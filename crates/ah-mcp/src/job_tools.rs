//! The `ah.job.*` tools: their schemas, their arguments, and the results a job
//! snapshot turns into.

use crate::server::{
    JOB_CANCEL_TOOL, JOB_RESULT_TOOL, JOB_START_TOOL, JOB_STATUS_TOOL, RISK_META_KEY,
};

use ah_plugin_api::CommandError;
use rmcp::model::{
    CallToolResult, ContentBlock, JsonObject, Meta, TaskSupport, Tool, ToolAnnotations,
    ToolExecution,
};
use serde_json::{Map, Value, json};

use crate::jobs::{JobRegistryError, JobSnapshot, JobStatus};

pub(crate) fn job_tools() -> Vec<Tool> {
    vec![
        job_tool(
            JOB_START_TOOL,
            "Start AIHelper job",
            "Start any published AIHelper tool without waiting for its result. Inspect the target tool risk metadata before calling. Retries are not generally safe.",
            json!({
                "type": "object",
                "properties": {
                    "tool": {"type": "string", "minLength": 1},
                    "arguments": {"type": "object"}
                },
                "required": ["tool", "arguments"],
                "additionalProperties": false
            }),
            (false, true, false, true),
            "critical",
            "Can invoke any published AIHelper tool, including destructive commands.",
        ),
        job_tool(
            JOB_STATUS_TOOL,
            "Inspect AIHelper job",
            "Return the current state of an AIHelper background job without waiting.",
            job_id_schema(),
            (true, false, true, false),
            "low",
            "Reads process-local job metadata only.",
        ),
        job_tool(
            JOB_RESULT_TOOL,
            "Read AIHelper job result",
            "Return ready=false immediately while running, or the repeatable terminal result.",
            job_id_schema(),
            (true, false, true, false),
            "low",
            "Reads a retained process-local job result only.",
        ),
        job_tool(
            JOB_CANCEL_TOOL,
            "Cancel AIHelper job",
            "Request cooperative cancellation and return the job's current terminal state immediately.",
            job_id_schema(),
            (false, true, true, false),
            "medium",
            "Requests cancellation of one active command; the handler may continue draining.",
        ),
    ]
}

pub(crate) fn job_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"job_id": {"type": "string", "minLength": 1}},
        "required": ["job_id"],
        "additionalProperties": false
    })
}

pub(crate) fn job_tool(
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: Value,
    annotations: (bool, bool, bool, bool),
    risk: &'static str,
    impact: &'static str,
) -> Tool {
    let mut tool = Tool::new(
        name,
        description,
        input_schema
            .as_object()
            .cloned()
            .expect("job tool input schema must be an object"),
    );
    tool.title = Some(title.to_owned());
    tool.annotations = Some(ToolAnnotations::from_raw(
        Some(title.to_owned()),
        Some(annotations.0),
        Some(annotations.1),
        Some(annotations.2),
        Some(annotations.3),
    ));
    tool.execution = Some(ToolExecution::new().with_task_support(TaskSupport::Forbidden));
    tool.meta = Some(Meta(Map::from_iter([(
        RISK_META_KEY.to_owned(),
        json!({
            "level": risk,
            "impact": impact,
            "effects": ["process_state"],
            "reversibility": if name == JOB_START_TOOL { "unknown" } else { "yes" }
        }),
    )])));
    tool
}

pub(crate) fn take_job_id(arguments: &mut JsonObject) -> Result<String, CommandError> {
    match arguments.remove("job_id") {
        Some(Value::String(job_id)) if !job_id.trim().is_empty() => Ok(job_id),
        _ => Err(job_argument_error(
            "job control requires a non-empty string property 'job_id'",
        )),
    }
}

pub(crate) fn job_snapshot_result(snapshot: &JobSnapshot, include_result: bool) -> CallToolResult {
    let mut data = Map::from_iter([
        ("job_id".to_owned(), Value::String(snapshot.job_id.clone())),
        ("tool".to_owned(), Value::String(snapshot.tool.clone())),
        (
            "status".to_owned(),
            Value::String(snapshot.status.as_str().to_owned()),
        ),
        ("draining".to_owned(), Value::Bool(snapshot.draining)),
    ]);
    if include_result {
        let ready = snapshot.status != JobStatus::Running;
        data.insert("ready".to_owned(), Value::Bool(ready));
        if let Some(response) = &snapshot.response {
            data.insert(
                "response".to_owned(),
                serde_json::to_value(response)
                    .expect("typed invocation response must always serialize"),
            );
        }
    }
    let data = Value::Object(data);
    let mut result = CallToolResult::structured(data.clone());
    result.content = vec![ContentBlock::text(
        serde_json::to_string(&data).expect("job result must always serialize"),
    )];
    result
}

pub(crate) fn job_argument_error(cause: impl Into<String>) -> CommandError {
    CommandError::new(
        Some("job".to_owned()),
        None,
        "INVALID_ARGUMENT",
        "Invalid job tool arguments",
        cause,
        2,
        false,
    )
}

pub(crate) fn job_registry_error(error: JobRegistryError) -> CommandError {
    match error {
        JobRegistryError::CapacityFull { capacity } => CommandError::new(
            Some("job".to_owned()),
            Some("job.start".to_owned()),
            "JOB_CAPACITY_FULL",
            "AIHelper job registry is full",
            format!("all {capacity} retained records are active or draining"),
            1,
            true,
        ),
        JobRegistryError::NotFound { job_id } => CommandError::new(
            Some("job".to_owned()),
            None,
            "JOB_NOT_FOUND",
            "AIHelper job was not found",
            format!("job '{job_id}' is unknown, expired, or evicted"),
            2,
            false,
        ),
    }
}
