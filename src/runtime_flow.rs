use std::{ffi::OsString, sync::Arc};

use ah_runtime::{
    PluginLoadReport, PluginManager, PluginSource,
    executor::{Executor, SequentialExecutor},
};

use crate::{
    ai,
    cli::{self, CliParseResult, RuntimeCommand},
    config::ConfigContext,
    error::AppError,
    output::{emit_muted_stderr, emit_warning},
    plugin_settings::PluginSettings,
    plugins,
};

pub(crate) fn run() -> Result<(), AppError> {
    let mut runtime = startup()?;
    let mut load_report = discovery(&runtime.config, &mut runtime.manager, &runtime.settings);
    let command = match routing(runtime.raw_args, &runtime.manager)? {
        RoutingOutcome::ExitSuccess => return Ok(()),
        RoutingOutcome::Command(command) => command,
    };
    render_discovery_diagnostics(&mut load_report, &command);
    execution(command, runtime.manager, runtime.settings)
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

fn startup() -> Result<RuntimeStartup, AppError> {
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    cli::apply_initial_cwd_from_raw_args(&raw_args)?;

    let config = ConfigContext::load()?;
    let settings = PluginSettings::load_from_path(config.paths().plugin_settings_file.clone())?;
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
) -> Result<(), AppError> {
    match command {
        RuntimeCommand::McpServe {
            max_queued,
            default_timeout_ms,
            options,
        } => execute_mcp_serve(manager, settings, max_queued, default_timeout_ms, options),
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
) -> Result<(), AppError> {
    let cwd = std::env::current_dir()
        .map_err(|source| AppError::cwd(std::path::PathBuf::from("."), source))?
        .to_string_lossy()
        .into_owned();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .map_err(|error| AppError::external("MCP_RUNTIME_FAILED", error.to_string()))?;
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
                .map_err(crate::map_runtime_error)?,
        );
        let config = ah_mcp::McpServerConfig::new(cwd, options.limit, default_timeout_ms)
            .map_err(|error| AppError::external("MCP_CONFIG_INVALID", error.to_string()))?;
        let server = ah_mcp::McpServer::new(manager, executor, config)
            .map_err(|error| AppError::external("MCP_SERVER_FAILED", error.to_string()))?;
        ah_mcp::serve_stdio(server)
            .await
            .map_err(|error| AppError::external("MCP_SERVER_FAILED", error.to_string()))
    })
}
