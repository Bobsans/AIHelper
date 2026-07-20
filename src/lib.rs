#![allow(clippy::result_large_err)]

pub mod ai;
pub mod cli;
pub mod commands;
pub mod config;
pub mod error;
pub(crate) mod event_log;
pub(crate) mod git_status;
pub(crate) mod host_commands;
pub mod output;
mod persistence;
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
    output::{OutputMode, TextFormatter, TextStyle},
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

    let plugins = collect_plugin_list_entries(manager, state_filter)?;

    match options.output {
        OutputMode::Text => {
            if plugins.is_empty() {
                println!("no plugins registered");
                return Ok(());
            }
            println!(
                "{}",
                render_plugins_table(&plugins, TextFormatter::stdout())
            );
        }
        OutputMode::Json => {
            println!("{}", serde_json::to_string_pretty(&plugins)?);
        }
    }
    Ok(())
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
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Text => {
            let style = if changed {
                TextStyle::Success
            } else {
                TextStyle::Warning
            };
            println!("{}", TextFormatter::stdout().paint(style, text_message));
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

fn render_plugins_table(plugins: &[PluginListEntry], formatter: TextFormatter) -> String {
    let domain_width = column_width(
        "DOMAIN",
        plugins.iter().map(|plugin| plugin.domain.as_str()),
    );
    let plugin_width = column_width(
        "PLUGIN",
        plugins.iter().map(|plugin| plugin.plugin_name.as_str()),
    );
    let source_width = column_width("SOURCE", plugins.iter().map(|plugin| plugin.source));
    let state_width = column_width("STATE", plugins.iter().map(|plugin| plugin.state));

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
        let source_style = if plugin.source == "dynamic" {
            TextStyle::Key
        } else {
            TextStyle::Muted
        };
        let state_style = if plugin.state == "enabled" {
            TextStyle::Success
        } else {
            TextStyle::Error
        };
        lines.push(format!(
            "{}  {}  {}  {}  {}",
            formatter.paint(TextStyle::Key, pad_column(&plugin.domain, domain_width)),
            pad_column(&plugin.plugin_name, plugin_width),
            formatter.paint(source_style, pad_column(plugin.source, source_width)),
            formatter.paint(state_style, pad_column(plugin.state, state_width)),
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
        RuntimeError::InvalidCommandCatalog { domain, reason } => AppError::external(
            "COMMAND_CATALOG_INVALID",
            format!("invalid typed command catalog for domain '{domain}': {reason}"),
        ),
        RuntimeError::TypedCommandNotFound(command) => AppError::external(
            "TYPED_COMMAND_NOT_FOUND",
            format!("typed command not found: {command}"),
        ),
        RuntimeError::TypedInvocation(message) => {
            AppError::external("TYPED_INVOCATION_FAILED", message)
        }
        RuntimeError::TypedResponseValidation { command, reason } => AppError::external(
            "OUTPUT_SCHEMA_VIOLATION",
            format!("typed command response failed validation for '{command}': {reason}"),
        ),
        RuntimeError::InvalidExecutionRequest(message) => {
            AppError::external("EXECUTION_REQUEST_INVALID", message)
        }
        RuntimeError::ExecutionQueueFull { capacity } => AppError::external(
            "EXECUTION_QUEUE_FULL",
            format!("typed execution queue is full (capacity {capacity})"),
        ),
        RuntimeError::ExecutionCancelled { request_id } => AppError::external(
            "EXECUTION_CANCELLED",
            format!("typed execution request '{request_id}' was cancelled"),
        ),
        RuntimeError::ExecutionTimeout { request_id } => AppError::external(
            "EXECUTION_TIMEOUT",
            format!("typed execution request '{request_id}' timed out"),
        ),
        RuntimeError::ExecutionDraining { request_id } => AppError::external(
            "EXECUTOR_DRAINING",
            format!("timed-out execution request '{request_id}' is still draining"),
        ),
        RuntimeError::ExecutionWorker(message) => {
            AppError::external("EXECUTION_WORKER_FAILED", message)
        }
        RuntimeError::ExecutionPanic { request_id } => AppError::external(
            "EXECUTION_HANDLER_PANIC",
            format!("typed execution handler panicked for request '{request_id}'"),
        ),
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
    mcp_exposed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_omission_reason: Option<&'static str>,
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
    use super::{PluginListEntry, render_plugins_table};
    use crate::output::TextFormatter;

    #[test]
    fn plugins_table_aligns_plain_text_columns() {
        let plugins = vec![
            plugin_entry("file", "builtin-file", "builtin", "enabled", "Read files"),
            plugin_entry(
                "postgres",
                "external-postgres",
                "dynamic",
                "disabled",
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
            "builtin",
            "enabled",
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
        source: &'static str,
        state: &'static str,
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
