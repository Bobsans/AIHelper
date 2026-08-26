//! What `ah mcp service` prints.
//!
//! `key=value` lines rather than prose, deliberately unstyled and greppable:
//! these commands are read by people debugging a service that will not start,
//! and as often by scripts.
//!
//! Separated from the output *types*, which the mechanism produces and which
//! travel to the crate with it.

use serde::Serialize;

use crate::{
    error::AppError,
    mcp_service::{
        model::{MutationOutput, UninstallOutput},
        output::StatusOutput,
    },
    output::Emitter,
};

pub(crate) fn mutation(value: &MutationOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    let text = render_mutation_text(value)?;
    emitter.value(value, |_| text)
}

pub(crate) fn uninstall(value: &UninstallOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    let text = render_uninstall_text(value)?;
    emitter.value(value, |_| text)
}

pub(crate) fn status(value: &StatusOutput, emitter: &mut Emitter) -> Result<(), AppError> {
    let text = render_status_text(value)?;
    emitter.value(value, |_| text)
}

/// The service commands report `key=value` lines rather than prose, so the text
/// rendering is deliberately unstyled and machine-greppable.
fn render_mutation_text(value: &MutationOutput) -> Result<String, AppError> {
    Ok([
        format!("command={}", value.command),
        format!("schema_version={}", value.schema_version),
        format!("changed={}", value.changed),
        format!("action={}", value.action),
        format!("service_id={}", value.service_id),
        format!("configuration_id={}", value.configuration_id),
        format!("task_path={}", value.task_path),
        format!("endpoint={}", value.endpoint),
        format!("registration={}", enum_json(&value.registration)?),
        format!("runtime={}", enum_json(&value.runtime)?),
    ]
    .join("\n"))
}

fn render_uninstall_text(value: &UninstallOutput) -> Result<String, AppError> {
    Ok([
        format!("command={}", value.command),
        format!("schema_version={}", value.schema_version),
        format!("changed={}", value.changed),
        format!("action={}", value.action),
        format!(
            "service_id={}",
            value
                .service_id
                .map(|value| value.to_string())
                .as_deref()
                .unwrap_or("null")
        ),
        format!(
            "configuration_id={}",
            value
                .configuration_id
                .map(|value| value.to_string())
                .as_deref()
                .unwrap_or("null")
        ),
        format!("task_path={}", value.task_path),
        format!("endpoint={}", value.endpoint.as_deref().unwrap_or("null")),
        format!("registration={}", value.registration),
        format!("runtime={}", enum_json(&value.runtime)?),
    ]
    .join("\n"))
}

fn render_status_text(value: &StatusOutput) -> Result<String, AppError> {
    let mut lines = vec![
        format!("command={}", value.command),
        format!("schema_version={}", value.schema_version),
        format!(
            "registration.status={}",
            enum_json(&value.registration.status)?
        ),
        format!("registration.task_path={}", value.registration.task_path),
        format!("scheduler.state={}", enum_json(&value.scheduler.state)?),
        format!("runtime.status={}", enum_json(&value.runtime.status)?),
        format!("readiness.status={}", enum_json(&value.readiness.status)?),
        format!("lifecycle.status={}", enum_json(&value.lifecycle.status)?),
        format!("drift.count={}", value.drift.len()),
    ];
    for drift in &value.drift {
        lines.push(format!(
            "drift={} kind={} expected={} actual={} code={}",
            drift.field,
            enum_json(&drift.kind)?,
            drift.expected.as_deref().unwrap_or("null"),
            drift.actual.as_deref().unwrap_or("null"),
            drift.diagnostic_code
        ));
    }
    Ok(lines.join("\n"))
}

fn enum_json<T: Serialize>(value: &T) -> Result<String, AppError> {
    Ok(serde_json::to_string(value)?.trim_matches('"').to_owned())
}
