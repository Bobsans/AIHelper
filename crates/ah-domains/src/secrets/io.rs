//! The effects a secrets command has: reading a passphrase from the terminal,
//! asking the running MCP server for a setup capability, and handing a URL to
//! the browser.

use std::collections::BTreeMap;

use ah_mcp::SecretSetupRequest;
use dialoguer::Password;
use serde::Deserialize;

use ah_error::AppError;
use ah_output::{Emitter, GlobalOptions};
use ah_secrets::{SecretKind, VaultStore};

use super::{
    domain::{SecretsCommand, invalid_setup_url, validate_setup_url, vault_error},
    output::render_setup_output,
};

#[derive(Deserialize)]
pub(super) struct SetupCapabilityResponse {
    setup_url: String,
}

pub(super) fn open_browser_setup(
    request: SecretSetupRequest,
    options: GlobalOptions,
) -> Result<(), AppError> {
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
    Emitter::stdio(&options).report(|_| render_setup_output(setup_url.as_str(), options.output))
}

pub(super) fn open_browser(url: &str) -> Result<(), AppError> {
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

pub(super) fn prompt_values(
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

pub(super) fn prompt_kind_values(
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
