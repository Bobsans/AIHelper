//! Rendering what a secrets command produced.

use ah_error::AppError;
use ah_output::OutputMode;
use ah_secrets::SecretMetadata;

use super::domain::{SecretsListOutput, SecretsOutput};

pub(super) fn render_setup_output(setup_url: &str, mode: OutputMode) -> Result<String, AppError> {
    match mode {
        OutputMode::Text => Ok(setup_url.to_owned()),
        OutputMode::Json => Ok(serde_json::to_string_pretty(
            &serde_json::json!({"setup_url": setup_url}),
        )?),
    }
}

pub(super) fn render_json(output: &SecretsOutput) -> Result<String, AppError> {
    let value = match output {
        SecretsOutput::Initialized => serde_json::json!({"initialized": true}),
        SecretsOutput::List(secrets) => serde_json::to_value(SecretsListOutput { secrets })?,
        SecretsOutput::Metadata(metadata) => serde_json::to_value(metadata)?,
    };
    Ok(serde_json::to_string_pretty(&value)?)
}

pub(super) fn render_text(output: &SecretsOutput) -> String {
    match output {
        SecretsOutput::Initialized => "secret vault initialized".to_owned(),
        SecretsOutput::List(secrets) if secrets.is_empty() => "no secrets configured".to_owned(),
        SecretsOutput::List(secrets) => secrets
            .iter()
            .map(render_metadata)
            .collect::<Vec<_>>()
            .join("\n"),
        SecretsOutput::Metadata(metadata) => render_metadata(metadata),
    }
}

pub(super) fn render_metadata(metadata: &SecretMetadata) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        metadata.id,
        metadata.kind,
        metadata.label,
        metadata.description.as_deref().unwrap_or("-")
    )
}
