//! Rendering what a secrets command produced.

use ah_error::AppError;
use ah_output::{OutputMode, TextFormatter, TextStyle};
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

pub(super) fn render_text(output: &SecretsOutput, formatter: TextFormatter) -> String {
    match output {
        SecretsOutput::Initialized => "secret vault initialized".to_owned(),
        SecretsOutput::List(secrets) if secrets.is_empty() => "no secrets configured".to_owned(),
        SecretsOutput::List(secrets) => render_list(secrets, formatter),
        SecretsOutput::Metadata(metadata) => render_metadata(metadata),
    }
}

fn render_list(secrets: &[SecretMetadata], formatter: TextFormatter) -> String {
    let id_width = secrets
        .iter()
        .map(|s| s.id.chars().count())
        .max()
        .unwrap_or(0);
    let kind_width = secrets
        .iter()
        .map(|s| s.kind.to_string().chars().count())
        .max()
        .unwrap_or(0);
    let label_width = secrets
        .iter()
        .map(|s| s.label.chars().count())
        .max()
        .unwrap_or(0);

    secrets
        .iter()
        .map(|secret| {
            format!(
                "{}  {}  {:<label_width$}  {}",
                formatter.paint(TextStyle::Key, format!("{:<id_width$}", secret.id)),
                formatter.paint(
                    TextStyle::Heading,
                    format!("{:<kind_width$}", secret.kind.to_string())
                ),
                secret.label,
                formatter.paint(
                    TextStyle::Muted,
                    secret.description.as_deref().unwrap_or("-")
                ),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;
    use ah_secrets::SecretKind;

    fn list() -> SecretsOutput {
        SecretsOutput::List(vec![
            SecretMetadata {
                id: "a".to_owned(),
                kind: SecretKind::Postgres,
                label: "База".to_owned(),
                description: None,
            },
            SecretMetadata {
                id: "long-id".to_owned(),
                kind: SecretKind::GitlabToken,
                label: "UCO GitLab access token".to_owned(),
                description: Some("Production".to_owned()),
            },
        ])
    }

    #[test]
    fn list_aligns_columns_using_plain_character_widths() {
        assert_eq!(
            render_text(&list(), TextFormatter::with_color(false)),
            "a        postgres      База                     -\n\
             long-id  gitlab-token  UCO GitLab access token  Production"
        );
    }

    #[test]
    fn list_colors_metadata_without_changing_alignment() {
        let output = list();
        let colored = render_text(&output, TextFormatter::with_color(true));
        assert!(colored.contains("\u{1b}[36ma      \u{1b}[0m"));
        assert!(colored.contains("\u{1b}[1;36mpostgres    \u{1b}[0m"));
        assert!(colored.contains("\u{1b}[2m-\u{1b}[0m"));
        let plain = ["\u{1b}[36m", "\u{1b}[1;36m", "\u{1b}[2m", "\u{1b}[0m"]
            .iter()
            .fold(colored, |text, code| text.replace(code, ""));
        assert_eq!(
            plain,
            render_text(&output, TextFormatter::with_color(false))
        );
    }

    #[test]
    fn empty_list_keeps_its_plain_message() {
        for color in [false, true] {
            assert_eq!(
                render_text(
                    &SecretsOutput::List(vec![]),
                    TextFormatter::with_color(color)
                ),
                "no secrets configured"
            );
        }
    }
}
