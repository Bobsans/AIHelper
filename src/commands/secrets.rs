use std::collections::BTreeMap;

use ah_mcp::SecretSetupRequest;
use dialoguer::Password;
use serde::{Deserialize, Serialize};

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
enum SecretsOutput {
    Initialized,
    List(Vec<SecretMetadata>),
    Metadata(SecretMetadata),
}

#[derive(Serialize)]
struct SecretsListOutput<'a> {
    secrets: &'a [SecretMetadata],
}

#[derive(Deserialize)]
struct SetupCapabilityResponse {
    setup_url: String,
}

pub(crate) fn execute(
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

fn browser_setup_request(request: &SecretsCommand) -> Option<SecretSetupRequest> {
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

fn open_browser_setup(request: SecretSetupRequest, options: GlobalOptions) -> Result<(), AppError> {
    let base_url =
        std::env::var("AH_MCP_HTTP_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".to_owned());
    let origin = reqwest::Url::parse(&base_url).map_err(|_| invalid_setup_url())?;
    if origin.scheme() != "http"
        || origin.host_str() != Some("127.0.0.1")
        || origin.port().is_none()
        || !origin.username().is_empty()
        || origin.password().is_some()
        || !matches!(origin.path(), "" | "/")
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err(invalid_setup_url());
    }
    let capability_endpoint = origin
        .join("/secrets/setup/capability")
        .map_err(|_| invalid_setup_url())?;
    let response = reqwest::blocking::Client::new()
        .post(capability_endpoint)
        .json(&request)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|_| {
            AppError::external(
                "VAULT_SETUP_UNAVAILABLE",
                "failed to create browser setup capability",
            )
        })?
        .json::<SetupCapabilityResponse>()
        .map_err(|_| {
            AppError::external(
                "VAULT_SETUP_UNAVAILABLE",
                "browser setup response was invalid",
            )
        })?;
    let setup_url = validate_setup_url(&origin, &response.setup_url)?;
    let _ = open_browser(setup_url.as_str());
    if !options.quiet {
        println!(
            "{}",
            render_setup_output(setup_url.as_str(), options.output)?
        );
    }
    Ok(())
}

fn validate_setup_url(origin: &reqwest::Url, setup_url: &str) -> Result<reqwest::Url, AppError> {
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

fn render_setup_output(setup_url: &str, mode: OutputMode) -> Result<String, AppError> {
    match mode {
        OutputMode::Text => Ok(setup_url.to_owned()),
        OutputMode::Json => Ok(serde_json::to_string_pretty(
            &serde_json::json!({"setup_url": setup_url}),
        )?),
    }
}

fn invalid_setup_url() -> AppError {
    AppError::external(
        "VAULT_SETUP_URL_INVALID",
        "AH_MCP_HTTP_URL must be an http://127.0.0.1:PORT origin",
    )
}

fn invalid_returned_setup_url() -> AppError {
    AppError::external(
        "VAULT_SETUP_URL_INVALID",
        "browser setup response did not contain a trusted setup URL",
    )
}

fn open_browser(url: &str) -> Result<(), AppError> {
    #[cfg(target_os = "windows")]
    let child = ah_plugin_api::noninteractive_command("rundll32")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    #[cfg(target_os = "macos")]
    let child = ah_plugin_api::noninteractive_command("open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let child = ah_plugin_api::noninteractive_command("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    child
        .map(|_| ())
        .map_err(|_| AppError::external("VAULT_SETUP_OPEN_FAILED", "failed to open browser setup"))
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
        SecretKind::GithubToken => &[("token", "GitHub personal access token", false)],
        SecretKind::GitlabToken => &[("token", "GitLab personal access token", false)],
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

    use crate::{
        output::OutputMode,
        secrets::{KeyProvider, SecretKind, VaultError, VaultStore},
    };

    use super::{
        SecretsCommand, apply, render_json, render_setup_output, render_text, validate_setup_url,
    };

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
