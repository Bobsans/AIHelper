use std::collections::BTreeMap;

use ah_mcp::SecretSetupRequest;
use serde::Serialize;

use super::{
    io::{open_browser_setup, prompt_values},
    output::{render_json, render_text},
};
use crate::{
    cli::GlobalOptions,
    config::ConfigContext,
    error::AppError,
    output::{Emitter, OutputMode},
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
        open: bool,
    },
    Edit {
        id: String,
        label: Option<String>,
        description: Option<String>,
        open: bool,
    },
    Remove {
        id: String,
    },
}

#[derive(Debug)]
pub(crate) enum SecretsOutput {
    Initialized,
    List(Vec<SecretMetadata>),
    Metadata(SecretMetadata),
}

#[derive(Serialize)]
pub(crate) struct SecretsListOutput<'a> {
    pub(super) secrets: &'a [SecretMetadata],
}

pub fn execute(
    config: &ConfigContext,
    request: SecretsCommand,
    options: GlobalOptions,
) -> Result<(), AppError> {
    if let Some(request) = browser_setup_request(&request) {
        return open_browser_setup(request, options);
    }
    let store = VaultStore::new(config, resolve_key_provider().map_err(vault_error)?);
    let values = prompt_values(&store, &request)?;
    let output = apply(&store, request, values)?;
    Emitter::stdio(&options).report(|_| match options.output {
        OutputMode::Text => Ok(render_text(&output)),
        OutputMode::Json => render_json(&output),
    })
}

pub(super) fn browser_setup_request(request: &SecretsCommand) -> Option<SecretSetupRequest> {
    match request {
        SecretsCommand::Add {
            id,
            kind,
            label,
            description,
            open: true,
        } => Some(SecretSetupRequest::Create {
            id: id.clone(),
            kind: kind.to_string(),
            label: label.clone(),
            description: description.clone(),
        }),
        SecretsCommand::Edit {
            id,
            label,
            description,
            open: true,
        } => Some(SecretSetupRequest::Edit {
            id: id.clone(),
            label: label.clone(),
            description: description.clone(),
        }),
        _ => None,
    }
}

pub(super) fn validate_setup_url(
    origin: &reqwest::Url,
    setup_url: &str,
) -> Result<reqwest::Url, AppError> {
    let setup_url = reqwest::Url::parse(setup_url).map_err(|_| invalid_returned_setup_url())?;
    if setup_url.scheme() != origin.scheme()
        || setup_url.host_str() != origin.host_str()
        || setup_url.port() != origin.port()
        || !setup_url.username().is_empty()
        || setup_url.password().is_some()
        || setup_url.path() != "/secrets/setup"
        || setup_url.fragment().is_some()
    {
        return Err(invalid_returned_setup_url());
    }
    let mut query = setup_url.query_pairs();
    match (query.next(), query.next()) {
        (Some((key, value)), None) if key == "capability" && !value.is_empty() => Ok(setup_url),
        _ => Err(invalid_returned_setup_url()),
    }
}

pub(super) fn invalid_setup_url() -> AppError {
    AppError::external(
        "VAULT_SETUP_URL_INVALID",
        "AH_MCP_HTTP_URL must be an http://127.0.0.1:PORT origin",
    )
}

pub(super) fn invalid_returned_setup_url() -> AppError {
    AppError::external(
        "VAULT_SETUP_URL_INVALID",
        "browser setup response did not contain a trusted setup URL",
    )
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
            open: _,
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
            open: _,
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

pub(super) fn missing_prompt_values() -> AppError {
    AppError::external("SECRET_PROMPT_FAILED", "secret fields were not provided")
}

pub(super) fn vault_error(error: VaultError) -> AppError {
    AppError::external(error.code(), error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use crate::{
        output::OutputMode,
        secrets::{KeyProvider, SecretKind, VaultError, VaultStore},
    };

    use super::{SecretsCommand, apply, validate_setup_url};
    use crate::commands::secrets::output::{render_json, render_setup_output, render_text};

    struct FixedKey;

    impl KeyProvider for FixedKey {
        fn load_or_create(&self) -> Result<[u8; 32], VaultError> {
            Ok([23; 32])
        }
    }

    #[test]
    fn setup_url_requires_the_configured_origin_and_exact_capability_shape() {
        let origin = reqwest::Url::parse("http://127.0.0.1:8787").unwrap();
        let valid = "http://127.0.0.1:8787/secrets/setup?capability=one-time-token";
        assert_eq!(validate_setup_url(&origin, valid).unwrap().as_str(), valid);

        for invalid in [
            "https://127.0.0.1:8787/secrets/setup?capability=token",
            "http://127.0.0.1:8788/secrets/setup?capability=token",
            "http://localhost:8787/secrets/setup?capability=token",
            "http://user@127.0.0.1:8787/secrets/setup?capability=token",
            "http://127.0.0.1:8787/not-setup?capability=token",
            "http://127.0.0.1:8787/secrets/setup?capability=token&extra=value",
            "http://127.0.0.1:8787/secrets/setup?capability=one&capability=two",
            "http://127.0.0.1:8787/secrets/setup?other=token",
            "http://127.0.0.1:8787/secrets/setup?capability=",
            "http://127.0.0.1:8787/secrets/setup?capability=token#fragment",
        ] {
            assert!(validate_setup_url(&origin, invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn rejected_setup_url_does_not_expose_its_capability() {
        let origin = reqwest::Url::parse("http://127.0.0.1:8787").unwrap();
        let secret = "do-not-log-this-capability";
        let error = validate_setup_url(
            &origin,
            &format!("http://127.0.0.1:8787/secrets/setup?capability={secret}&extra=rejected"),
        )
        .unwrap_err();

        assert!(!error.to_string().contains(secret));
    }

    #[test]
    fn setup_output_prints_the_url_for_headless_use() {
        let url = "http://127.0.0.1:8787/secrets/setup?capability=one-time-token";

        assert_eq!(render_setup_output(url, OutputMode::Text).unwrap(), url);
        let json: serde_json::Value =
            serde_json::from_str(&render_setup_output(url, OutputMode::Json).unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"setup_url": url}));
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
                open: false,
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
                open: false,
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
