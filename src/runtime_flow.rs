use std::{ffi::OsString, sync::Arc, time::Instant};

use ah_runtime::{
    PluginLoadReport, PluginManager, PluginSource,
    executor::{Executor, SequentialExecutor},
};

use crate::{
    ai,
    cli::{self, CliParseResult, RuntimeCommand},
    config::ConfigContext,
    error::AppError,
    event_log::{EventDiagnostic, EventLogger, SystemEventSeverity},
    output::{emit_muted_stderr, emit_warning},
    plugin_settings::PluginSettings,
    plugins,
};

pub(crate) fn run() -> Result<(), AppError> {
    let started = Instant::now();
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    let logged_argv = raw_args
        .iter()
        .skip(1)
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if let Err(error) = cli::apply_initial_cwd_from_raw_args(&raw_args) {
        let logger = EventLogger::new();
        record_app_system_error(
            logger.as_ref(),
            "startup",
            &error,
            serde_json::json!({"argv": logged_argv}),
        );
        return Err(error);
    }
    let logger = EventLogger::new().map(Arc::new);
    let mut runtime = startup(raw_args, logger.as_deref(), &logged_argv)?;
    let mut load_report = discovery(&runtime.config, &mut runtime.manager, &runtime.settings);
    record_discovery_events(logger.as_deref(), &load_report);
    let command = match routing(runtime.raw_args, &runtime.manager) {
        Ok(RoutingOutcome::ExitSuccess) => {
            if let Some(logger) = &logger {
                logger.record_cli_command(
                    successful_exit_command_name(&logged_argv),
                    logged_argv,
                    started.elapsed(),
                    None,
                );
            }
            return Ok(());
        }
        Ok(RoutingOutcome::Command(command)) => command,
        Err(error) => {
            record_app_system_error(
                logger.as_deref(),
                "cli_parse",
                &error,
                serde_json::json!({"argv": logged_argv}),
            );
            return Err(error);
        }
    };
    render_discovery_diagnostics(&mut load_report, &command);
    let command_name = command_log_name(&command, &runtime.manager);
    let result = execution(command, runtime.manager, runtime.settings, logger.clone());
    if let Some(logger) = &logger {
        logger.record_cli_command(
            &command_name,
            logged_argv,
            started.elapsed(),
            result.as_ref().err(),
        );
    }
    result
}

struct RuntimeStartup {
    raw_args: Vec<OsString>,
    config: ConfigContext,
    settings: PluginSettings,
    manager: PluginManager,
}

enum RoutingOutcome {
    ExitSuccess,
    Command(RuntimeCommand),
}

fn successful_exit_command_name(argv: &[String]) -> &'static str {
    if argv
        .iter()
        .any(|argument| matches!(argument.as_str(), "--version" | "-V"))
    {
        "version"
    } else {
        "help"
    }
}

fn startup(
    raw_args: Vec<OsString>,
    logger: Option<&EventLogger>,
    logged_argv: &[String],
) -> Result<RuntimeStartup, AppError> {
    let config = ConfigContext::load().inspect_err(|error| {
        record_app_system_error(
            logger,
            "config",
            error,
            serde_json::json!({"argv": logged_argv}),
        );
    })?;
    let settings = PluginSettings::load_from_path(config.paths().plugin_settings_file.clone())
        .inspect_err(|error| {
            record_app_system_error(
                logger,
                "config",
                error,
                serde_json::json!({"argv": logged_argv}),
            );
        })?;
    let mut manager = PluginManager::new();
    manager.reserve_dynamic_domains(["ai", "plugins", "mcp"]);
    for plugin in plugins::builtins() {
        manager.register_builtin(plugin);
    }

    Ok(RuntimeStartup {
        raw_args,
        config,
        settings,
        manager,
    })
}

fn discovery(
    config: &ConfigContext,
    manager: &mut PluginManager,
    settings: &PluginSettings,
) -> PluginLoadReport {
    let plugin_dirs = config.paths().plugin_dirs.clone();
    let load_report = crate::load_dynamic_plugins_from_dirs(manager, &plugin_dirs);
    manager.set_disabled_domains(settings.disabled_domains().cloned());
    load_report
}

fn routing(raw_args: Vec<OsString>, manager: &PluginManager) -> Result<RoutingOutcome, AppError> {
    let plugin_metadata = manager.list_enabled_plugins();
    match cli::parse_runtime_command(raw_args, &plugin_metadata)? {
        CliParseResult::ExitSuccess => Ok(RoutingOutcome::ExitSuccess),
        CliParseResult::Command(command) => Ok(RoutingOutcome::Command(command)),
    }
}

fn render_discovery_diagnostics(load_report: &mut PluginLoadReport, command: &RuntimeCommand) {
    if crate::command_is_quiet(command) {
        return;
    }

    load_report
        .conflicts
        .sort_by(|left, right| left.domain.cmp(&right.domain));
    load_report
        .warnings
        .sort_by(|left, right| left.path.cmp(&right.path));
    for warning in &load_report.warnings {
        emit_warning(format!(
            "skipped plugin {}: {}",
            warning.path.display(),
            warning.error
        ));
    }
    for conflict in &load_report.conflicts {
        emit_warning(format!(
            "domain '{}' conflict: {}",
            conflict.domain, conflict.reason
        ));
        emit_muted_stderr(format!(
            "  keeping {} plugin '{}', ignored {} plugin '{}'",
            plugin_source_name(conflict.winner_source),
            conflict.winner.plugin_name,
            plugin_source_name(conflict.loser_source),
            conflict.loser.plugin_name
        ));
    }
}

fn record_discovery_events(logger: Option<&EventLogger>, load_report: &PluginLoadReport) {
    let Some(logger) = logger else {
        return;
    };
    for warning in &load_report.warnings {
        logger.record_system_event(
            "plugin_discovery",
            SystemEventSeverity::Warning,
            EventDiagnostic::new(
                "PLUGIN_LOAD_WARNING",
                "dynamic plugin was skipped during discovery",
                0,
            )
            .with_cause(warning.error.clone()),
            serde_json::json!({"path": warning.path.to_string_lossy()}),
        );
    }
    for conflict in &load_report.conflicts {
        logger.record_system_event(
            "plugin_discovery",
            SystemEventSeverity::Warning,
            EventDiagnostic::new(
                "PLUGIN_DOMAIN_CONFLICT",
                "plugin domain conflict was resolved",
                0,
            )
            .with_identity(Some(conflict.domain.clone()), None)
            .with_cause(conflict.reason.clone()),
            serde_json::json!({
                "winner": conflict.winner.plugin_name,
                "winner_source": plugin_source_name(conflict.winner_source),
                "loser": conflict.loser.plugin_name,
                "loser_source": plugin_source_name(conflict.loser_source),
            }),
        );
    }
}

fn record_app_system_error(
    logger: Option<&EventLogger>,
    component: &str,
    error: &AppError,
    context: serde_json::Value,
) {
    if let Some(logger) = logger {
        logger.record_system_event(
            component,
            SystemEventSeverity::Error,
            EventDiagnostic::from_app_error(error),
            context,
        );
    }
}

fn command_log_name(command: &RuntimeCommand, manager: &PluginManager) -> String {
    match command {
        RuntimeCommand::McpServe { .. } => "mcp.serve".to_owned(),
        RuntimeCommand::PluginsList { .. } => "plugins.list".to_owned(),
        RuntimeCommand::PluginsEnable { .. } => "plugins.enable".to_owned(),
        RuntimeCommand::PluginsDisable { .. } => "plugins.disable".to_owned(),
        RuntimeCommand::PluginsReset { .. } => "plugins.reset".to_owned(),
        RuntimeCommand::AiInfo { .. } => "ai.info".to_owned(),
        RuntimeCommand::Invoke { domain, argv, .. } => {
            resolve_invocation_command(manager, domain, argv)
        }
    }
}

fn resolve_invocation_command(manager: &PluginManager, domain: &str, argv: &[String]) -> String {
    let prefix = format!("{domain}.");
    let catalog_match = manager
        .command_catalog_for_domain(domain)
        .ok()
        .flatten()
        .and_then(|catalog| {
            catalog
                .commands
                .into_iter()
                .filter_map(|descriptor| {
                    let suffix = descriptor.id.strip_prefix(&prefix)?;
                    let segments = suffix.split('.').collect::<Vec<_>>();
                    let matches = segments.len() <= argv.len()
                        && segments
                            .iter()
                            .zip(argv)
                            .all(|(segment, argument)| segment == argument);
                    matches.then_some((segments.len(), descriptor.id))
                })
                .max_by_key(|(segment_count, _)| *segment_count)
                .map(|(_, command)| command)
        });
    catalog_match.unwrap_or_else(|| {
        argv.first()
            .map(|operation| format!("{domain}.{operation}"))
            .unwrap_or_else(|| domain.to_owned())
    })
}

fn plugin_source_name(source: PluginSource) -> &'static str {
    match source {
        PluginSource::Builtin => "builtin",
        PluginSource::Dynamic => "dynamic",
    }
}

fn execution(
    command: RuntimeCommand,
    manager: PluginManager,
    mut settings: PluginSettings,
    logger: Option<Arc<EventLogger>>,
) -> Result<(), AppError> {
    match command {
        RuntimeCommand::McpServe {
            max_queued,
            default_timeout_ms,
            options,
        } => execute_mcp_serve(
            manager,
            settings,
            max_queued,
            default_timeout_ms,
            options,
            logger,
        ),
        RuntimeCommand::PluginsList {
            state_filter,
            options,
        } => crate::execute_plugins_list(&manager, state_filter, options),
        RuntimeCommand::PluginsEnable { domain, options } => {
            crate::execute_plugins_enable(&manager, &mut settings, &domain, options)
        }
        RuntimeCommand::PluginsDisable { domain, options } => {
            crate::execute_plugins_disable(&manager, &mut settings, &domain, options)
        }
        RuntimeCommand::PluginsReset {
            domain,
            all,
            options,
        } => crate::execute_plugins_reset(&manager, &mut settings, domain.as_deref(), all, options),
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
                .map_err(crate::map_runtime_error)?;
            crate::handle_response(response, options.output, options.quiet)
        }
    }
}

fn execute_mcp_serve(
    manager: PluginManager,
    settings: PluginSettings,
    max_queued: usize,
    default_timeout_ms: u64,
    options: cli::GlobalOptions,
    logger: Option<Arc<EventLogger>>,
) -> Result<(), AppError> {
    let cwd = std::env::current_dir()
        .map_err(|source| AppError::cwd(std::path::PathBuf::from("."), source))
        .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_server", error))?
        .to_string_lossy()
        .into_owned();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .map_err(|error| AppError::external("MCP_RUNTIME_FAILED", error.to_string()))
        .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_server", error))?;
    runtime.block_on(async move {
        let settings = Arc::new(std::sync::Mutex::new(settings));
        let manager = Arc::new_cyclic(|weak| {
            let mut manager = manager;
            for plugin in crate::host_commands::builtins(weak.clone(), Arc::clone(&settings)) {
                manager.register_host_builtin(plugin);
            }
            manager
        });
        let executor: Arc<dyn Executor> = Arc::new(
            SequentialExecutor::new(Arc::clone(&manager), max_queued)
                .map_err(crate::map_runtime_error)
                .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_server", error))?,
        );
        let config = ah_mcp::McpServerConfig::new(cwd, options.limit, default_timeout_ms)
            .map_err(|error| AppError::external("MCP_CONFIG_INVALID", error.to_string()))
            .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_server", error))?;
        let mut server = ah_mcp::McpServer::new(manager, executor, config)
            .map_err(|error| AppError::external("MCP_SERVER_FAILED", error.to_string()))
            .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_server", error))?;
        if let Some(logger) = logger.clone() {
            let event_sink: Arc<dyn ah_mcp::EventSink> = logger;
            server = server.with_event_sink(event_sink);
        }
        ah_mcp::serve_stdio(server)
            .await
            .map_err(|error| AppError::external("MCP_SERVER_FAILED", error.to_string()))
            .map_err(|error| record_mcp_system_error(logger.as_deref(), "mcp_transport", error))
    })
}

fn record_mcp_system_error(
    logger: Option<&EventLogger>,
    component: &str,
    error: AppError,
) -> AppError {
    record_app_system_error(logger, component, &error, serde_json::json!({}));
    error
}

#[cfg(test)]
mod tests {
    use ah_runtime::PluginManager;

    use super::resolve_invocation_command;

    #[test]
    fn command_logging_prefers_longest_catalog_descriptor() {
        let mut manager = PluginManager::new();
        for plugin in crate::plugins::builtins() {
            manager.register_builtin(plugin);
        }

        assert_eq!(
            resolve_invocation_command(
                &manager,
                "git",
                &["tag".to_owned(), "create".to_owned(), "v1".to_owned()],
            ),
            "git.tag.create"
        );
        assert_eq!(
            resolve_invocation_command(
                &manager,
                "file",
                &["read".to_owned(), "sample.txt".to_owned()],
            ),
            "file.read"
        );
    }

    #[test]
    fn command_logging_falls_back_without_catalog_match() {
        let manager = PluginManager::new();
        assert_eq!(
            resolve_invocation_command(&manager, "legacy", &["inspect".to_owned()]),
            "legacy.inspect"
        );
        assert_eq!(
            resolve_invocation_command(&manager, "legacy", &[]),
            "legacy"
        );
    }
}
