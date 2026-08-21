use std::collections::BTreeMap;

use dialoguer::Password;
use serde::Serialize;

use crate::{
    cli::GlobalOptions,
    config::ConfigContext,
    error::AppError,
    output::OutputMode,
    secrets::{
        NewSecret, SecretKind, SecretMetadata, VaultError, VaultStore, resolve_key_provider,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsCommand {
    Init,
    List {
        kind: Option<SecretKind>,
    },
    Add {
        id: String,
        kind: SecretKind,
        label: Option<String>,
        description: Option<String>,
    },
    Edit {
        id: String,
        label: Option<String>,
        description: Option<String>,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug)]
enum SecretsOutput {
    Initialized,
    List(Vec<SecretMetadata>),
    Metadata(SecretMetadata),
}

#[derive(Serialize)]
struct SecretsListOutput<'a> {
    secrets: &'a [SecretMetadata],
}

pub(crate) fn execute(
    config: &ConfigContext,
    request: SecretsCommand,
    options: GlobalOptions,
) -> Result<(), AppError> {
    let store = VaultStore::new(config, resolve_key_provider().map_err(vault_error)?);
    let values = prompt_values(&store, &request)?;
    let output = apply(&store, request, values)?;
    if options.quiet {
        return Ok(());
    }
    println!(
        "{}",
        match options.output {
            OutputMode::Text => render_text(&output),
            OutputMode::Json => render_json(&output)?,
        }
    );
    Ok(())
}

fn apply(
    store: &VaultStore,
    request: SecretsCommand,
    values: Option<BTreeMap<String, String>>,
) -> Result<SecretsOutput, AppError> {
    match request {
        SecretsCommand::Init => {
            store.initialize().map_err(vault_error)?;
            Ok(SecretsOutput::Initialized)
        }
        SecretsCommand::List { kind } => {
            let mut secrets = store.list_metadata().map_err(vault_error)?;
            if let Some(kind) = kind {
                secrets.retain(|secret| secret.kind == kind);
            }
            Ok(SecretsOutput::List(secrets))
        }
        SecretsCommand::Add {
            id,
            kind,
            label,
            description,
        } => {
            let values = values.ok_or_else(missing_prompt_values)?;
            let secret = NewSecret::new(id.clone(), label.unwrap_or(id), kind, values)
                .with_description(description);
            Ok(SecretsOutput::Metadata(
                store.put(secret).map_err(vault_error)?,
            ))
        }
        SecretsCommand::Edit {
            id,
            label,
            description,
        } => {
            let existing = store.resolve(&id).map_err(vault_error)?;
            let values = values.ok_or_else(missing_prompt_values)?;
            let secret = NewSecret::new(
                id,
                label.unwrap_or(existing.metadata.label),
                existing.metadata.kind,
                values,
            )
            .with_description(description.or(existing.metadata.description));
            Ok(SecretsOutput::Metadata(
                store.replace(secret).map_err(vault_error)?,
            ))
        }
        SecretsCommand::Remove { id } => Ok(SecretsOutput::Metadata(
            store.remove(&id).map_err(vault_error)?,
        )),
    }
}

fn prompt_values(
    store: &VaultStore,
    request: &SecretsCommand,
) -> Result<Option<BTreeMap<String, String>>, AppError> {
    match request {
        SecretsCommand::Add { kind, .. } => prompt_kind_values(*kind, None).map(Some),
        SecretsCommand::Edit { id, .. } => {
            let existing = store.resolve(id).map_err(vault_error)?;
            prompt_kind_values(existing.metadata.kind, Some(&existing.values)).map(Some)
        }
        _ => Ok(None),
    }
}

fn prompt_kind_values(
    kind: SecretKind,
    existing: Option<&BTreeMap<String, String>>,
) -> Result<BTreeMap<String, String>, AppError> {
    let fields: &[(&str, &str, bool)] = match kind {
        SecretKind::Postgres => &[("password", "PostgreSQL password", false)],
        SecretKind::HttpBasic => &[
            ("username", "HTTP basic username", false),
            ("password", "HTTP basic password", false),
        ],
        SecretKind::SshKey => &[
            ("private_key", "SSH private key", false),
            ("passphrase", "SSH key passphrase (optional)", true),
        ],
    };
    let mut values = BTreeMap::new();
    for (name, prompt, optional) in fields {
        let value = Password::new()
            .with_prompt(*prompt)
            .allow_empty_password(existing.is_some() || *optional)
            .interact()
            .map_err(|_| {
                AppError::external("SECRET_PROMPT_FAILED", "failed to read secret field")
            })?;
        if value.is_empty() {
            if let Some(value) = existing.and_then(|values| values.get(*name)) {
                values.insert((*name).to_owned(), value.clone());
            }
        } else {
            values.insert((*name).to_owned(), value);
        }
    }
    Ok(values)
}

fn render_json(output: &SecretsOutput) -> Result<String, AppError> {
    let value = match output {
        SecretsOutput::Initialized => serde_json::json!({"initialized": true}),
        SecretsOutput::List(secrets) => serde_json::to_value(SecretsListOutput { secrets })?,
        SecretsOutput::Metadata(metadata) => serde_json::to_value(metadata)?,
    };
    Ok(serde_json::to_string_pretty(&value)?)
}

fn render_text(output: &SecretsOutput) -> String {
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

fn render_metadata(metadata: &SecretMetadata) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        metadata.id,
        metadata.kind,
        metadata.label,
        metadata.description.as_deref().unwrap_or("-")
    )
}

fn missing_prompt_values() -> AppError {
    AppError::external("SECRET_PROMPT_FAILED", "secret fields were not provided")
}

fn vault_error(error: VaultError) -> AppError {
    AppError::external(error.code(), error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use crate::secrets::{KeyProvider, SecretKind, VaultError, VaultStore};

    use super::{SecretsCommand, apply, render_json, render_text};

    struct FixedKey;

    impl KeyProvider for FixedKey {
        fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
            Ok([23; 32])
        }
    }

    #[test]
    fn prompt_independent_add_and_edit_never_render_secret_values() {
        let directory = tempfile::tempdir().unwrap();
        let store = VaultStore::at(directory.path(), Arc::new(FixedKey));
        store.initialize().unwrap();
        let original = "original-cli-secret";
        let replacement = "replacement-cli-secret";

        let added = apply(
            &store,
            SecretsCommand::Add {
                id: "billing".to_owned(),
                kind: SecretKind::Postgres,
                label: Some("Billing".to_owned()),
                description: Some("Production billing database".to_owned()),
            },
            Some(BTreeMap::from([(
                "password".to_owned(),
                original.to_owned(),
            )])),
        )
        .unwrap();
        let edited = apply(
            &store,
            SecretsCommand::Edit {
                id: "billing".to_owned(),
                label: None,
                description: None,
            },
            Some(BTreeMap::from([(
                "password".to_owned(),
                replacement.to_owned(),
            )])),
        )
        .unwrap();

        for output in [
            render_json(&added).unwrap(),
            render_text(&added),
            render_json(&edited).unwrap(),
            render_text(&edited),
        ] {
            assert!(!output.contains(original));
            assert!(!output.contains(replacement));
            assert!(output.contains("billing"));
        }
    }
}
