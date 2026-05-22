pub mod ai;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub mod output;
pub mod plugin_settings;
pub mod plugins;
mod runtime_flow;
pub mod safety;

use std::path::{Path, PathBuf};

use ah_plugin_api::{ErrorDiagnostic, InvocationResponse, RequiredTool};
use ah_runtime::{PluginManager, PluginSource, RuntimeError};
use serde::Serialize;

use crate::{
    cli::{PluginStateFilter, RuntimeCommand},
    error::AppError,
    output::OutputMode,
    plugin_settings::PluginSettings,
};

pub fn run() -> Result<(), AppError> {
    runtime_flow::run()
}

fn execute_plugins_list(
    manager: &PluginManager,
    state_filter: Option<PluginStateFilter>,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }

    let mut plugins = manager
        .list_registered_plugins()
        .into_iter()
        .map(|plugin| PluginListEntry {
            abi_version: plugin.metadata.abi_version,
            description: plugin.metadata.description,
            domain: plugin.metadata.domain,
            plugin_name: plugin.metadata.plugin_name,
            required_tools: plugin.metadata.required_tools,
            source: plugin_source_label(plugin.source),
            state: plugin_state_label(plugin.enabled),
        })
        .collect::<Vec<_>>();
    if let Some(filter) = state_filter {
        plugins.retain(|plugin| matches_plugin_filter(plugin, filter));
    }

    match options.output {
        OutputMode::Text => {
            if plugins.is_empty() {
                println!("no plugins registered");
                return Ok(());
            }
            for plugin in plugins {
                println!(
                    "{} ({}) [{}|{}] - {}",
                    plugin.domain,
                    plugin.plugin_name,
                    plugin.source,
                    plugin.state,
                    plugin.description
                );
            }
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&plugins)?);
        }
    }
    Ok(())
}

fn execute_plugins_enable(
    manager: &PluginManager,
    settings: &mut PluginSettings,
    domain: &str,
    options: cli::GlobalOptions,
) -> Result<(), AppError> {
    let normalized_domain = validate_known_domain(manager, domain)?;
    let changed = settings.enable_domain(&normalized_domain)?;
    if changed {
        settings.save()?;
    }
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
    let changed = settings.disable_domain(&normalized_domain)?;
    if changed {
        settings.save()?;
    }
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
        let changed = settings.clear_all();
        if changed {
            settings.save()?;
        }
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
    let changed = settings.reset_domain(&normalized_domain)?;
    if changed {
        settings.save()?;
    }
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
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Text => {
            println!("{text_message}");
        }
        OutputMode::Json => {
            let payload = PluginStateMutationOutput {
                command,
                changed,
                config_path: normalize_path(settings.path()),
                disabled_domains: settings.disabled_domains().cloned().collect(),
                domain: domain.map(str::to_owned),
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn matches_plugin_filter(plugin: &PluginListEntry, filter: PluginStateFilter) -> bool {
    match filter {
        PluginStateFilter::Enabled => plugin.state == "enabled",
        PluginStateFilter::Disabled => plugin.state == "disabled",
    }
}

fn plugin_source_label(source: PluginSource) -> &'static str {
    match source {
        PluginSource::Builtin => "builtin",
        PluginSource::Dynamic => "dynamic",
    }
}

fn plugin_state_label(enabled: bool) -> &'static str {
    if enabled { "enabled" } else { "disabled" }
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

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn handle_response(
    response: InvocationResponse,
    output_mode: OutputMode,
    quiet: bool,
) -> Result<(), AppError> {
    if response.success {
        if quiet {
            return Ok(());
        }
        if let Some(message) = response.message {
            match output_mode {
                OutputMode::Text => println!("{message}"),
                OutputMode::Json => println!("{message}"),
            }
        }
        return Ok(());
    }

    if let Some(diagnostic) = response.diagnostic {
        return Err(AppError::from_diagnostic(diagnostic));
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
    match error {
        RuntimeError::DomainNotFound(domain) => AppError::external(
            "DOMAIN_NOT_FOUND",
            format!("unknown command domain: {domain}"),
        ),
        RuntimeError::DomainDisabled(domain) => AppError::external(
            "DOMAIN_DISABLED",
            format!("plugin domain is disabled: {domain}"),
        ),
        RuntimeError::DependencyMissing {
            domain,
            operation,
            tool,
            reason,
        } => AppError::from_diagnostic(ErrorDiagnostic::new(
            Some(domain),
            operation,
            "DEPENDENCY_MISSING",
            format!("required external tool not found: {tool}"),
            reason,
            1,
        )),
        RuntimeError::LibraryLoad { path, source } => AppError::external(
            "PLUGIN_LIBRARY_LOAD_FAILED",
            format!(
                "failed to load plugin library '{}': {source}",
                path.display()
            ),
        ),
        RuntimeError::SymbolLoad { path, source } => AppError::external(
            "PLUGIN_SYMBOL_LOAD_FAILED",
            format!(
                "failed to load plugin entrypoint '{}': {source}",
                path.display()
            ),
        ),
        RuntimeError::AbiVersionMismatch {
            path,
            found,
            expected,
        } => AppError::external(
            "PLUGIN_ABI_MISMATCH",
            format!(
                "plugin '{}' has incompatible ABI version {found}; expected {expected}",
                path.display()
            ),
        ),
        RuntimeError::ApiVersionMismatch {
            path,
            found_major,
            found_minor,
            supported_major,
            supported_minor,
        } => AppError::external(
            "PLUGIN_API_MISMATCH",
            format!(
                "plugin '{}' requires unsupported Plugin API version {found_major}.{found_minor}; host supports {supported_major}.{supported_minor}",
                path.display()
            ),
        ),
        RuntimeError::InvalidMetadata { path, reason } => AppError::external(
            "PLUGIN_METADATA_INVALID",
            format!(
                "plugin '{}' returned invalid metadata: {reason}",
                path.display()
            ),
        ),
        RuntimeError::Invocation(message) => {
            AppError::external("PLUGIN_INVOCATION_FAILED", message)
        }
        RuntimeError::ResponseParse(message) => {
            AppError::external("PLUGIN_RESPONSE_PARSE_FAILED", message)
        }
    }
}

fn command_is_quiet(command: &RuntimeCommand) -> bool {
    match command {
        RuntimeCommand::PluginsList { options, .. } => options.quiet,
        RuntimeCommand::PluginsEnable { options, .. } => options.quiet,
        RuntimeCommand::PluginsDisable { options, .. } => options.quiet,
        RuntimeCommand::PluginsReset { options, .. } => options.quiet,
        RuntimeCommand::AiInfo { options, .. } => options.quiet,
        RuntimeCommand::Invoke { options, .. } => options.quiet,
    }
}

#[derive(Debug, Clone, Serialize)]
struct PluginListEntry {
    plugin_name: String,
    domain: String,
    description: String,
    abi_version: u32,
    required_tools: Vec<RequiredTool>,
    source: &'static str,
    state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
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
    for dir in dirs {
        let report = manager.load_dynamic_plugins_from_dir(dir);
        merged.loaded += report.loaded;
        merged.skipped += report.skipped;
        merged.warnings.extend(report.warnings);
        merged.conflicts.extend(report.conflicts);
    }
    merged
}
