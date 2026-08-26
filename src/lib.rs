pub mod ai;
pub mod cli;
pub mod commands;
pub mod config;
mod entry;
pub mod error;
pub(crate) mod event_log;
pub(crate) mod git_status;
pub(crate) mod host_commands;
pub mod mcp_service;
pub mod output;
mod persistence;
pub mod plugin_settings;
pub mod plugins;
#[cfg(test)]
mod reference_docs;
mod runtime_flow;
pub mod safety;
pub mod secrets;
#[cfg(test)]
mod snapshots;
pub(crate) mod updater;

use std::{path::PathBuf, sync::Arc};

use ah_plugin_api::{InvocationResponse, RequiredTool, ResolvedSecret};
use ah_runtime::{
    PluginManager, PluginSource, RuntimeError, SecretResolver, SecretResolverError, core,
};
use serde::Serialize;

use crate::{
    cli::{PluginStateFilter, RuntimeCommand},
    error::AppError,
    output::{Emitter, OutputMode, TextFormatter, TextStyle},
    plugin_settings::PluginSettings,
};

pub fn run() -> Result<(), AppError> {
    secrets::capture_startup_master_key()
        .map_err(|error| AppError::external(error.code(), error.to_string()))?;
    runtime_flow::run()
}

struct RuntimeVaultKeyProvider;

impl secrets::KeyProvider for RuntimeVaultKeyProvider {
    fn load_or_create(&self) -> Result<[u8; 32], secrets::VaultError> {
        secrets::resolve_key_provider()?.load_or_create()
    }
}

fn runtime_vault(config: &config::ConfigContext) -> Arc<secrets::VaultStore> {
    Arc::new(secrets::VaultStore::new(
        config,
        Box::new(RuntimeVaultKeyProvider),
    ))
}

impl SecretResolver for secrets::VaultStore {
    fn resolve(&self, id: &str) -> Result<ResolvedSecret, SecretResolverError> {
        let secret =
            secrets::VaultStore::resolve(self, id).map_err(|error| match error.code() {
                "VAULT_SECRET_NOT_FOUND" | "VAULT_NOT_INITIALIZED" => SecretResolverError::NotFound,
                "VAULT_KEY_UNAVAILABLE" => SecretResolverError::VaultKeyUnavailable,
                _ => SecretResolverError::VaultLocked,
            })?;
        Ok(ResolvedSecret {
            id: secret.metadata.id,
            kind: secret.metadata.kind.to_string(),
            values: secret.values,
        })
    }
}

fn execute_plugins_list(
    manager: &PluginManager,
    state_filter: Option<PluginStateFilter>,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    let plugins = collect_plugin_list_entries(manager, state_filter)?;

    Emitter::stdio(&options).value(&plugins, |formatter| {
        if plugins.is_empty() {
            "no plugins registered".to_owned()
        } else {
            render_plugins_table(&plugins, formatter)
        }
    })
}

fn collect_plugin_list_entries(
    manager: &PluginManager,
    state_filter: Option<PluginStateFilter>,
) -> Result<Vec<PluginListEntry>, AppError> {
    let mut plugins = manager
        .list_registered_plugins()
        .into_iter()
        .map(|plugin| {
            let mcp_exposed = manager
                .command_catalog_for_domain(&plugin.metadata.domain)
                .map_err(map_runtime_error)?
                .is_some();
            Ok(PluginListEntry {
                abi_version: plugin.metadata.abi_version,
                description: plugin.metadata.description,
                domain: plugin.metadata.domain,
                mcp_exposed,
                mcp_omission_reason: (!mcp_exposed)
                    .then_some("plugin does not provide typed_commands_v1"),
                plugin_name: plugin.metadata.plugin_name,
                required_tools: plugin.metadata.required_tools,
                source: plugin_source_label(plugin.source),
                state: plugin_state_label(plugin.enabled),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    if let Some(filter) = state_filter {
        plugins.retain(|plugin| matches_plugin_filter(plugin, filter));
    }
    Ok(plugins)
}

fn execute_plugins_enable(
    manager: &PluginManager,
    settings: &mut PluginSettings,
    domain: &str,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    let normalized_domain = validate_known_domain(manager, domain)?;
    let changed = settings.update(|candidate| candidate.enable_domain(&normalized_domain))?;
    manager.set_disabled_domains(settings.disabled_domains().cloned());
    render_plugin_state_mutation(
        "plugins.enable",
        settings,
        Some(&normalized_domain),
        changed,
        if changed {
            format!("enabled plugin domain '{}'", normalized_domain)
        } else {
            format!("plugin domain '{}' is already enabled", normalized_domain)
        },
        options,
    )
}

fn execute_plugins_disable(
    manager: &PluginManager,
    settings: &mut PluginSettings,
    domain: &str,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    let normalized_domain = validate_known_domain(manager, domain)?;
    let changed = settings.update(|candidate| candidate.disable_domain(&normalized_domain))?;
    manager.set_disabled_domains(settings.disabled_domains().cloned());
    render_plugin_state_mutation(
        "plugins.disable",
        settings,
        Some(&normalized_domain),
        changed,
        if changed {
            format!("disabled plugin domain '{}'", normalized_domain)
        } else {
            format!("plugin domain '{}' is already disabled", normalized_domain)
        },
        options,
    )
}

fn execute_plugins_reset(
    manager: &PluginManager,
    settings: &mut PluginSettings,
    domain: Option<&str>,
    all: bool,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    if all {
        let changed = settings.update(|candidate| Ok(candidate.clear_all()))?;
        manager.set_disabled_domains(settings.disabled_domains().cloned());
        return render_plugin_state_mutation(
            "plugins.reset",
            settings,
            None,
            changed,
            if changed {
                "reset all plugin domain overrides".to_owned()
            } else {
                "no plugin domain overrides to reset".to_owned()
            },
            options,
        );
    }

    let Some(raw_domain) = domain else {
        return Err(AppError::invalid_argument(
            "missing domain for plugins reset (or use --all)",
        ));
    };
    let normalized_domain = validate_known_domain(manager, raw_domain)?;
    let changed = settings.update(|candidate| candidate.reset_domain(&normalized_domain))?;
    manager.set_disabled_domains(settings.disabled_domains().cloned());
    render_plugin_state_mutation(
        "plugins.reset",
        settings,
        Some(&normalized_domain),
        changed,
        if changed {
            format!(
                "reset plugin domain '{}' to default state (enabled)",
                normalized_domain
            )
        } else {
            format!("plugin domain '{}' has no override", normalized_domain)
        },
        options,
    )
}

fn render_plugin_state_mutation(
    command: &'static str,
    settings: &PluginSettings,
    domain: Option<&str>,
    changed: bool,
    text_message: String,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    let payload = PluginStateMutationOutput {
        command,
        changed,
        config_path: core::forward_slashes(settings.path()),
        disabled_domains: settings.disabled_domains().cloned().collect(),
        domain: domain.map(str::to_owned),
    };

    Emitter::stdio(&options).value(&payload, |formatter| {
        let style = if changed {
            TextStyle::Success
        } else {
            TextStyle::Warning
        };
        formatter.paint(style, text_message)
    })
}

fn matches_plugin_filter(plugin: &PluginListEntry, filter: PluginStateFilter) -> bool {
    match filter {
        PluginStateFilter::Enabled => plugin.state == PluginStateLabel::Enabled,
        PluginStateFilter::Disabled => plugin.state == PluginStateLabel::Disabled,
    }
}

fn render_plugins_table(plugins: &[PluginListEntry], formatter: TextFormatter) -> String {
    let domain_width = column_width(
        "DOMAIN",
        plugins.iter().map(|plugin| plugin.domain.as_str()),
    );
    let plugin_width = column_width(
        "PLUGIN",
        plugins.iter().map(|plugin| plugin.plugin_name.as_str()),
    );
    let source_width = column_width(
        "SOURCE",
        plugins.iter().map(|plugin| plugin.source.as_str()),
    );
    let state_width = column_width("STATE", plugins.iter().map(|plugin| plugin.state.as_str()));

    let mut lines = Vec::with_capacity(plugins.len() + 1);
    lines.push(format!(
        "{}  {}  {}  {}  {}",
        formatter.paint(TextStyle::Heading, pad_column("DOMAIN", domain_width)),
        formatter.paint(TextStyle::Heading, pad_column("PLUGIN", plugin_width)),
        formatter.paint(TextStyle::Heading, pad_column("SOURCE", source_width)),
        formatter.paint(TextStyle::Heading, pad_column("STATE", state_width)),
        formatter.paint(TextStyle::Heading, "DESCRIPTION")
    ));

    for plugin in plugins {
        let source_style = if plugin.source == PluginSourceLabel::Dynamic {
            TextStyle::Key
        } else {
            TextStyle::Muted
        };
        let state_style = if plugin.state == PluginStateLabel::Enabled {
            TextStyle::Success
        } else {
            TextStyle::Error
        };
        lines.push(format!(
            "{}  {}  {}  {}  {}",
            formatter.paint(TextStyle::Key, pad_column(&plugin.domain, domain_width)),
            pad_column(&plugin.plugin_name, plugin_width),
            formatter.paint(
                source_style,
                pad_column(plugin.source.as_str(), source_width)
            ),
            formatter.paint(state_style, pad_column(plugin.state.as_str(), state_width)),
            plugin.description
        ));
    }

    lines.join("\n")
}

fn column_width<'a>(heading: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values
        .map(str::chars)
        .map(Iterator::count)
        .fold(heading.chars().count(), usize::max)
}

fn pad_column(value: &str, width: usize) -> String {
    let padding = width.saturating_sub(value.chars().count());
    format!("{value}{}", " ".repeat(padding))
}

fn plugin_source_label(source: PluginSource) -> PluginSourceLabel {
    match source {
        PluginSource::Builtin => PluginSourceLabel::Builtin,
        PluginSource::Dynamic => PluginSourceLabel::Dynamic,
    }
}

fn plugin_state_label(enabled: bool) -> PluginStateLabel {
    if enabled {
        PluginStateLabel::Enabled
    } else {
        PluginStateLabel::Disabled
    }
}

fn validate_known_domain(manager: &PluginManager, domain: &str) -> Result<String, AppError> {
    let normalized = plugin_settings::normalize_domain(domain)?;
    let known_domain = manager
        .list_plugins()
        .into_iter()
        .any(|plugin| plugin.domain.eq_ignore_ascii_case(&normalized));
    if !known_domain {
        return Err(AppError::invalid_argument(format!(
            "unknown plugin domain: {}",
            domain.trim()
        )));
    }
    Ok(normalized)
}

fn handle_response(
    response: InvocationResponse,
    output_mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if response.success {
        if let Some(message) = response.message {
            // A plugin already rendered for the requested mode, so the message
            // is emitted as-is either way.
            Emitter::stdio(&cli::GlobalOptions {
                output: output_mode,
                quiet,
                limit: None,
            })
            .report(|_| Ok(message))?;
        }
        return Ok(());
    }

    if let Some(diagnostic) = response.diagnostic {
        return Err(AppError::from_diagnostic(*diagnostic));
    }

    let code = response
        .error_code
        .unwrap_or_else(|| "PLUGIN_EXECUTION_FAILED".to_owned());
    let message = response
        .error_message
        .unwrap_or_else(|| "plugin execution failed".to_owned());
    Err(AppError::external(code, message))
}

fn map_runtime_error(error: RuntimeError) -> AppError {
    // `RuntimeError::diagnostic` is the whole taxonomy; the host only overrides
    // the one variant that has a richer console rendering (suggestions, usage).
    match error {
        RuntimeError::DomainNotFound(domain) => AppError::unknown_command(domain, None),
        other => AppError::from_diagnostic(other.diagnostic()),
    }
}

fn command_is_quiet(command: &RuntimeCommand) -> bool {
    match command {
        RuntimeCommand::McpServe { options, .. } => options.quiet,
        RuntimeCommand::PluginsList { options, .. } => options.quiet,
        RuntimeCommand::PluginsEnable { options, .. } => options.quiet,
        RuntimeCommand::PluginsDisable { options, .. } => options.quiet,
        RuntimeCommand::PluginsReset { options, .. } => options.quiet,
        RuntimeCommand::AiInfo { options, .. } => options.quiet,
        RuntimeCommand::Ai { options, .. } => options.quiet,
        RuntimeCommand::Secrets { options, .. } => options.quiet,
        RuntimeCommand::Upgrade { options, .. } => options.quiet,
        RuntimeCommand::Invoke { options, .. } => options.quiet,
    }
}

// The `plugins.list` payload. A doc comment here would be published as the
// schema `description`, which the catalog does not carry for payload roots.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginsListOutput {
    pub(crate) plugins: Vec<PluginListEntry>,
}

// Both were `&'static str` whose legal values existed only in the hand-written
// schema; as enums the type and the published schema cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PluginSourceLabel {
    Builtin,
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PluginStateLabel {
    Enabled,
    Disabled,
}

impl PluginSourceLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Dynamic => "dynamic",
        }
    }
}

impl PluginStateLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PluginListEntry {
    plugin_name: String,
    domain: String,
    description: String,
    abi_version: u32,
    required_tools: Vec<RequiredTool>,
    source: PluginSourceLabel,
    state: PluginStateLabel,
    mcp_exposed: bool,
    // Absent rather than null for an MCP-exposed plugin, so the published
    // schema must not require it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("x-omissible" = true))]
    mcp_omission_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
struct PluginStateMutationOutput {
    command: &'static str,
    domain: Option<String>,
    changed: bool,
    config_path: String,
    disabled_domains: Vec<String>,
}

fn load_dynamic_plugins_from_dirs(
    manager: &mut PluginManager,
    dirs: &[PathBuf],
) -> ah_runtime::PluginLoadReport {
    let mut merged = ah_runtime::PluginLoadReport::default();
    // Configured directories are ordered from highest to lowest priority,
    // while the plugin manager intentionally gives the last loaded dynamic
    // plugin precedence for a domain.
    for dir in dirs.iter().rev() {
        let report = manager.load_dynamic_plugins_from_dir(dir);
        merged.loaded += report.loaded;
        merged.skipped += report.skipped;
        merged.warnings.extend(report.warnings);
        merged.conflicts.extend(report.conflicts);
    }
    merged
}

#[cfg(test)]
mod tests {
    use ah_runtime::RuntimeError;

    use super::{
        PluginListEntry, PluginSourceLabel, PluginStateLabel, map_runtime_error,
        render_plugins_table,
    };
    use crate::output::TextFormatter;

    #[test]
    fn runtime_secret_errors_keep_codes_and_redact_credential_ids() {
        let credential_id = "runtime-private-credential-id";
        let unexpected_kind = "runtime-unexpected-private-kind";
        let accepted_kind = "runtime-accepted-private-kind";
        let errors = [
            (
                RuntimeError::SecretNotFound {
                    command: "http.get".to_owned(),
                    slot: "basic".to_owned(),
                    id: credential_id.to_owned(),
                },
                "SECRET_NOT_FOUND",
            ),
            (
                RuntimeError::SecretKindMismatch {
                    command: "http.get".to_owned(),
                    slot: "basic".to_owned(),
                    id: credential_id.to_owned(),
                    kind: unexpected_kind.to_owned(),
                    accepted_kinds: vec![accepted_kind.to_owned()],
                },
                "SECRET_KIND_MISMATCH",
            ),
            (
                RuntimeError::VaultLocked {
                    command: "http.get".to_owned(),
                    slot: "basic".to_owned(),
                    id: credential_id.to_owned(),
                },
                "VAULT_LOCKED",
            ),
            (
                RuntimeError::VaultKeyUnavailable {
                    command: "http.get".to_owned(),
                    slot: "basic".to_owned(),
                    id: credential_id.to_owned(),
                },
                "VAULT_KEY_UNAVAILABLE",
            ),
        ];

        for (runtime_error, expected_code) in errors {
            let error = map_runtime_error(runtime_error);
            assert_eq!(error.code(), expected_code);
            assert!(!error.detail_message().contains(credential_id));
            assert!(!error.detail_message().contains(unexpected_kind));
            assert!(!error.detail_message().contains(accepted_kind));
        }
    }

    #[test]
    fn plugins_table_aligns_plain_text_columns() {
        let plugins = vec![
            plugin_entry(
                "file",
                "builtin-file",
                PluginSourceLabel::Builtin,
                PluginStateLabel::Enabled,
                "Read files",
            ),
            plugin_entry(
                "postgres",
                "external-postgres",
                PluginSourceLabel::Dynamic,
                PluginStateLabel::Disabled,
                "Query databases",
            ),
        ];

        let rendered = render_plugins_table(&plugins, TextFormatter::with_color(false));

        assert_eq!(
            rendered,
            "DOMAIN    PLUGIN             SOURCE   STATE     DESCRIPTION\n\
             file      builtin-file       builtin  enabled   Read files\n\
             postgres  external-postgres  dynamic  disabled  Query databases"
        );
    }

    #[test]
    fn plugins_table_applies_styles_after_padding() {
        let plugins = vec![plugin_entry(
            "http",
            "builtin-http",
            PluginSourceLabel::Builtin,
            PluginStateLabel::Enabled,
            "HTTP helpers",
        )];

        let rendered = render_plugins_table(&plugins, TextFormatter::with_color(true));

        assert!(rendered.contains("\u{1b}[1;36mDOMAIN\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mhttp  \u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[32menabled\u{1b}[0m"));
    }

    fn plugin_entry(
        domain: &str,
        plugin_name: &str,
        source: PluginSourceLabel,
        state: PluginStateLabel,
        description: &str,
    ) -> PluginListEntry {
        PluginListEntry {
            plugin_name: plugin_name.to_owned(),
            domain: domain.to_owned(),
            description: description.to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            source,
            state,
            mcp_exposed: false,
            mcp_omission_reason: Some("test fixture"),
        }
    }
}
