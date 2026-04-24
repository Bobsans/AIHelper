pub mod ai;
pub mod cli;
pub mod commands;
pub mod error;
pub mod output;
pub mod plugin_settings;
pub mod plugins;
pub mod safety;

use std::path::{Path, PathBuf};

use ah_plugin_api::InvocationResponse;
use ah_runtime::{PluginManager, PluginSource, RuntimeError};
use serde::Serialize;

use crate::{
    cli::{CliParseResult, PluginStateFilter, RuntimeCommand},
    error::AppError,
    output::OutputMode,
    plugin_settings::PluginSettings,
};

pub fn run() -> Result<(), AppError> {
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    cli::apply_initial_cwd_from_raw_args(&raw_args)?;

    let mut plugin_settings = PluginSettings::load()?;
    let mut manager = PluginManager::new();
    for plugin in plugins::builtins() {
        manager.register_builtin(plugin);
    }

    let plugin_dir = resolve_plugin_dir()?;
    let load_report = manager.load_dynamic_plugins_from_dir(&plugin_dir);
    manager.set_disabled_domains(plugin_settings.disabled_domains().cloned());

    let plugin_metadata = manager.list_enabled_plugins();
    let command = match cli::parse_runtime_command(raw_args, &plugin_metadata)? {
        CliParseResult::ExitSuccess => return Ok(()),
        CliParseResult::Command(command) => command,
    };
    if !command_is_quiet(&command) {
        for warning in &load_report.warnings {
            eprintln!(
                "warning: skipped plugin {}: {}",
                warning.path.display(),
                warning.error
            );
        }
    }

    match command {
        RuntimeCommand::PluginsList {
            state_filter,
            options,
        } => execute_plugins_list(&manager, state_filter, options),
        RuntimeCommand::PluginsEnable { domain, options } => {
            execute_plugins_enable(&manager, &mut plugin_settings, &domain, options)
        }
        RuntimeCommand::PluginsDisable { domain, options } => {
            execute_plugins_disable(&manager, &mut plugin_settings, &domain, options)
        }
        RuntimeCommand::PluginsReset {
            domain,
            all,
            options,
        } => execute_plugins_reset(
            &manager,
            &mut plugin_settings,
            domain.as_deref(),
            all,
            options,
        ),
        RuntimeCommand::AiInfo { domain, options } => {
            ai::execute_info(&manager, domain.as_deref(), options)
        }
        RuntimeCommand::Invoke {
            domain,
            argv,
            options,
        } => {
            let response = manager
                .invoke(&domain, argv, options.to_wire())
                .map_err(map_runtime_error)?;
            handle_response(response, options.output, options.quiet)
        }
    }
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

fn resolve_plugin_dir() -> Result<PathBuf, AppError> {
    let executable_path = std::env::current_exe().map_err(|source| {
        AppError::invalid_argument(format!("failed to resolve executable path: {source}"))
    })?;
    plugin_dir_from_executable_path(&executable_path)
}

fn plugin_dir_from_executable_path(executable_path: &Path) -> Result<PathBuf, AppError> {
    let executable_dir = executable_path.parent().ok_or_else(|| {
        AppError::invalid_argument(format!(
            "failed to resolve executable directory for '{}'",
            executable_path.display()
        ))
    })?;
    Ok(executable_dir.join("plugins"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_dir_is_next_to_executable() {
        let executable = PathBuf::from_iter(["opt", "aihelper", "ah"]);
        let plugin_dir =
            plugin_dir_from_executable_path(&executable).expect("plugin dir should resolve");
        assert_eq!(
            plugin_dir,
            PathBuf::from_iter(["opt", "aihelper", "plugins"])
        );
    }
}
