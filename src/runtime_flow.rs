use std::{
    ffi::{OsStr, OsString},
    sync::Arc,
    time::{Duration, Instant},
};

use ah_plugin_api::CommandCatalog;
use ah_runtime::{
    InvocationOutcome, PluginLoadReport, PluginManager, PluginSource,
    executor::{Executor, ParallelExecutor},
};

use crate::{
    ai,
    cli::{self, CliParseResult, RuntimeCommand},
    config::ConfigContext,
    error::{AppError, CommandSuggestion, FollowUpSuggestion, suggested_subcommand},
    event_log::{EventDiagnostic, EventLogger, SystemEventSeverity},
    mcp_service::{
        command::EarlyRoute,
        model::ExitKind,
        runner::{ManagedPreflight, ManagedRunner},
    },
    output::{emit_muted_stderr, emit_warning},
    plugin_settings::PluginSettings,
    plugins,
};

const MCP_RUNTIME_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub(crate) fn run() -> Result<(), AppError> {
    let started = Instant::now();
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    if is_installed_smoke_fast_path(&raw_args) {
        println!("ah {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if !is_updater_mcp_restore_fast_path(&raw_args) {
        match crate::updater::recovery::recover_before_startup(is_managed_serve_request(&raw_args))?
        {
            crate::updater::recovery::EarlyRecoveryOutcome::Continue => {}
            crate::updater::recovery::EarlyRecoveryOutcome::RecoveryLaunched => {
                emit_warning("update recovery started; rerun the command after recovery completes");
                return Ok(());
            }
        }
    }
    if is_version_fast_path(&raw_args) {
        println!("ah {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let logged_raw_args = cli::redact_secret_command_argv(&raw_args);
    let logged_argv = logged_raw_args
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
    match crate::updater::command::route(&raw_args)? {
        crate::updater::command::EarlyUpgradeRoute::NotUpgrade => {}
        crate::updater::command::EarlyUpgradeRoute::ExitSuccess => return Ok(()),
        crate::updater::command::EarlyUpgradeRoute::Execute { request, options } => {
            return crate::updater::execute(request, options);
        }
    }
    let managed_runner = match crate::mcp_service::command::route(&raw_args)? {
        EarlyRoute::NotManaged => None,
        EarlyRoute::ExitSuccess => return Ok(()),
        EarlyRoute::Service(command) => {
            return crate::mcp_service::lifecycle::execute(command);
        }
        EarlyRoute::ManagedServe { definition_path } => {
            match ManagedRunner::preflight(&definition_path)? {
                ManagedPreflight::AlreadyRunning => return Ok(()),
                ManagedPreflight::Ready(runner) => Some(Arc::new(*runner)),
            }
        }
    };
    let logger = EventLogger::new().map(Arc::new);
    let mut runtime = match startup(raw_args, logger.as_deref(), &logged_argv) {
        Ok(runtime) => runtime,
        Err(error) => {
            mark_managed_failure(&managed_runner, ExitKind::StartupFailure, &error);
            return Err(error);
        }
    };
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
            mark_managed_failure(&managed_runner, ExitKind::StartupFailure, &error);
            return Err(error);
        }
    };
    render_discovery_diagnostics(&mut load_report, &command);
    let command_name = command_log_name(&command, &runtime.manager);
    let result = execution(
        command,
        runtime.config,
        runtime.manager,
        runtime.settings,
        logger.clone(),
        managed_runner,
    );
    if let Some(logger) = &logger {
        logger.record_cli_command_with_outcome(
            &command_name,
            logged_argv,
            started.elapsed(),
            result.as_ref().ok().and_then(|outcome| outcome.as_ref()),
            result.as_ref().err(),
        );
    }
    result.map(|_| ())
}

fn is_installed_smoke_fast_path(raw_args: &[OsString]) -> bool {
    std::env::var_os("AH_UPDATER_INSTALLED_SMOKE").as_deref() == Some(OsStr::new("1"))
        && is_version_fast_path(raw_args)
}

fn is_updater_mcp_restore_fast_path(raw_args: &[OsString]) -> bool {
    std::env::var_os("AH_UPDATER_MCP_RESTORE").as_deref() == Some(OsStr::new("1"))
        && raw_args.len() == 5
        && raw_args[1] == "--json"
        && raw_args[2] == "mcp"
        && raw_args[3] == "service"
        && matches!(raw_args[4].to_str(), Some("install" | "start" | "status"))
}

fn is_managed_serve_request(raw_args: &[OsString]) -> bool {
    raw_args
        .windows(2)
        .any(|pair| pair[0] == "mcp" && pair[1] == "serve")
        && raw_args.iter().any(|argument| {
            argument == "--managed-config"
                || argument
                    .to_str()
                    .is_some_and(|value| value.starts_with("--managed-config="))
        })
}

fn is_version_fast_path(raw_args: &[OsString]) -> bool {
    raw_args.len() == 2
        && raw_args[1]
            .to_str()
            .is_some_and(|argument| matches!(argument, "--version" | "-V"))
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
    manager.reserve_dynamic_domains(["ai", "plugins", "mcp", "secrets", "upgrade"]);
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
        RuntimeCommand::Secrets { request, .. } => match request {
            crate::commands::secrets::SecretsCommand::Init => "secrets.init",
            crate::commands::secrets::SecretsCommand::List { .. } => "secrets.list",
            crate::commands::secrets::SecretsCommand::Add { .. } => "secrets.add",
            crate::commands::secrets::SecretsCommand::Edit { .. } => "secrets.edit",
            crate::commands::secrets::SecretsCommand::Remove { .. } => "secrets.remove",
        }
        .to_owned(),
        RuntimeCommand::Upgrade { .. } => "upgrade.check".to_owned(),
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
    config: ConfigContext,
    manager: PluginManager,
    mut settings: PluginSettings,
    logger: Option<Arc<EventLogger>>,
    managed_runner: Option<Arc<ManagedRunner>>,
) -> Result<Option<InvocationOutcome>, AppError> {
    match command {
        RuntimeCommand::McpServe {
            transport,
            port,
            max_active,
            default_timeout_ms,
            options,
        } => execute_mcp_serve(McpServeConfig {
            config,
            manager,
            settings,
            transport,
            port,
            max_active,
            default_timeout_ms,
            options,
            logger,
            managed_runner,
        })
        .map(|_| None),
        RuntimeCommand::PluginsList {
            state_filter,
            options,
        } => crate::execute_plugins_list(&manager, state_filter, options).map(|_| None),
        RuntimeCommand::PluginsEnable { domain, options } => {
            crate::execute_plugins_enable(&manager, &mut settings, &domain, options).map(|_| None)
        }
        RuntimeCommand::PluginsDisable { domain, options } => {
            crate::execute_plugins_disable(&manager, &mut settings, &domain, options).map(|_| None)
        }
        RuntimeCommand::PluginsReset {
            domain,
            all,
            options,
        } => crate::execute_plugins_reset(&manager, &mut settings, domain.as_deref(), all, options)
            .map(|_| None),
        RuntimeCommand::AiInfo { domain, options } => {
            ai::execute_info(&manager, domain.as_deref(), options).map(|_| None)
        }
        RuntimeCommand::Secrets { request, options } => {
            crate::commands::secrets::execute(&config, request, options).map(|_| None)
        }
        RuntimeCommand::Upgrade { request, options } => {
            crate::updater::execute(request, options).map(|_| None)
        }
        RuntimeCommand::Invoke {
            domain,
            argv,
            options,
        } => {
            let plugin_metadata = manager.list_enabled_plugins();
            let command_catalog = manager.command_catalog_for_domain(&domain).ok().flatten();
            let observation = manager
                .invoke_observed(&domain, argv, options.to_wire())
                .map_err(|error| match error {
                    ah_runtime::RuntimeError::DomainNotFound(domain) => {
                        let suggestion =
                            crate::cli::suggest_top_level_command(&domain, &plugin_metadata);
                        AppError::unknown_command(domain, suggestion)
                    }
                    other => crate::map_runtime_error(other),
                })?;
            crate::handle_response(observation.response, options.output, options.quiet).map_err(
                |error| decorate_invocation_error(error, &domain, command_catalog.as_ref()),
            )?;
            Ok(observation.outcome)
        }
    }
}

fn decorate_invocation_error(
    error: AppError,
    domain: &str,
    catalog: Option<&CommandCatalog>,
) -> AppError {
    if error.code() != "INVALID_ARGUMENT" {
        return error;
    }
    let detail = error.detail_message();
    let Some(candidate) = suggested_subcommand(&detail) else {
        return error;
    };
    let suggestion_description = catalog.and_then(|catalog| {
        catalog
            .commands
            .iter()
            .find(|command| command.id.rsplit('.').next() == Some(candidate))
            .map(|command| command.title.clone())
    });
    let follow_up = (candidate == "version" && domain != "version").then(|| {
        FollowUpSuggestion::new(
            "To show the AIHelper version, run:",
            CommandSuggestion::new("ah --version", None),
        )
    });
    error.with_suggestion_context(suggestion_description, follow_up)
}

struct McpServeConfig {
    config: ConfigContext,
    manager: PluginManager,
    settings: PluginSettings,
    transport: cli::McpTransport,
    port: u16,
    max_active: usize,
    default_timeout_ms: u64,
    options: cli::GlobalOptions,
    logger: Option<Arc<EventLogger>>,
    managed_runner: Option<Arc<ManagedRunner>>,
}

fn execute_mcp_serve(config: McpServeConfig) -> Result<(), AppError> {
    let (transport, port, max_active, default_timeout_ms, options) =
        if let Some(runner) = &config.managed_runner {
            if config.transport != cli::McpTransport::Http {
                return Err(AppError::invalid_argument(
                    "managed MCP configuration requires HTTP transport",
                ));
            }
            let definition = runner.definition();
            let mut managed_options = config.options;
            managed_options.limit = definition.server.limit;
            (
                cli::McpTransport::Http,
                definition.endpoint.port,
                definition.server.max_active,
                definition.server.default_timeout_ms,
                managed_options,
            )
        } else {
            (
                config.transport,
                config.port,
                config.max_active,
                config.default_timeout_ms,
                config.options,
            )
        };
    let cwd = std::env::current_dir()
        .map_err(|source| AppError::cwd(std::path::PathBuf::from("."), source))
        .map_err(|error| record_mcp_system_error(config.logger.as_deref(), "mcp_server", error));
    let cwd = match cwd {
        Ok(cwd) => cwd,
        Err(error) => {
            mark_managed_failure(&config.managed_runner, ExitKind::StartupFailure, &error);
            return Err(error);
        }
    }
    .to_string_lossy()
    .into_owned();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(max_active.saturating_add(16))
        .build()
        .map_err(|error| AppError::external("MCP_RUNTIME_FAILED", error.to_string()))
        .map_err(|error| record_mcp_system_error(config.logger.as_deref(), "mcp_server", error));
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            mark_managed_failure(&config.managed_runner, ExitKind::StartupFailure, &error);
            return Err(error);
        }
    };
    let runner_after_serve = config.managed_runner.clone();
    let (result, shutdown_grace) = runtime.block_on(async move {
        let setup = async {
            let vault = Arc::new(crate::secrets::VaultStore::new(
                &config.config,
                crate::secrets::resolve_key_provider()
                    .map_err(|error| AppError::external(error.code(), error.to_string()))?,
            ));
            let settings = Arc::new(std::sync::Mutex::new(config.settings));
            let manager = Arc::new_cyclic(|weak| {
                let mut manager = config.manager;
                for plugin in crate::host_commands::builtins(
                    weak.clone(),
                    Arc::clone(&settings),
                    Arc::clone(&vault),
                ) {
                    manager.register_host_builtin(plugin);
                }
                manager
            });
            let executor: Arc<dyn Executor> = Arc::new(
                ParallelExecutor::new(Arc::clone(&manager), max_active)
                    .map_err(crate::map_runtime_error)
                    .map_err(|error| {
                        record_mcp_system_error(config.logger.as_deref(), "mcp_server", error)
                    })?,
            );
            let config_server =
                ah_mcp::McpServerConfig::new(cwd, options.limit, default_timeout_ms)
                    .map_err(|error| AppError::external("MCP_CONFIG_INVALID", error.to_string()))
                    .map_err(|error| {
                        record_mcp_system_error(config.logger.as_deref(), "mcp_server", error)
                    })?;
            let mut server = ah_mcp::McpServer::new(manager, executor, config_server)
                .map_err(|error| AppError::external("MCP_SERVER_FAILED", error.to_string()))
                .map_err(|error| {
                    record_mcp_system_error(config.logger.as_deref(), "mcp_server", error)
                })?;
            if let Some(logger) = config.logger.clone() {
                let event_sink: Arc<dyn ah_mcp::EventSink> = logger;
                server = server.with_event_sink(event_sink);
            }
            Ok::<_, AppError>(match transport {
                cli::McpTransport::Stdio => {
                    ah_mcp::serve_stdio_bounded(server, MCP_RUNTIME_SHUTDOWN_GRACE).await
                }
                cli::McpTransport::Http => {
                    if let Some(runner) = config.managed_runner.clone() {
                        let instance_id = runner.instance_id();
                        let pid = runner.pid();
                        ah_mcp::serve_http_bounded_with_identity_and_listener(
                            server,
                            port,
                            env!("CARGO_PKG_VERSION"),
                            instance_id,
                            pid,
                            MCP_RUNTIME_SHUTDOWN_GRACE,
                            move || {
                                runner.mark_ready().map_err(|error| {
                                    ah_mcp::McpAdapterError::Service(error.detail_message())
                                })
                            },
                        )
                        .await
                    } else {
                        ah_mcp::serve_http_bounded_with_version(
                            server,
                            port,
                            env!("CARGO_PKG_VERSION"),
                            MCP_RUNTIME_SHUTDOWN_GRACE,
                        )
                        .await
                    }
                }
            })
        }
        .await;
        match setup {
            Ok(outcome) => {
                let (transport_result, remaining_grace) = outcome.into_parts();
                let result = transport_result
                    .map_err(map_mcp_transport_error)
                    .map_err(|error| {
                        record_mcp_system_error(config.logger.as_deref(), "mcp_transport", error)
                    });
                (result, remaining_grace)
            }
            Err(error) => (Err(error), MCP_RUNTIME_SHUTDOWN_GRACE),
        }
    });
    shutdown_runtime(runtime, shutdown_grace);
    if let Some(runner) = runner_after_serve {
        match &result {
            Ok(()) => {
                runner.mark_stopping()?;
                runner.mark_stopped()?;
            }
            Err(error) => {
                runner.mark_failed(ExitKind::RuntimeFailure, error.exit_code(), error.code())?
            }
        }
    }
    result
}

fn map_mcp_transport_error(error: ah_mcp::McpAdapterError) -> AppError {
    let code = if matches!(&error, ah_mcp::McpAdapterError::ShutdownTimeout { .. }) {
        "MCP_SHUTDOWN_TIMEOUT"
    } else {
        "MCP_SERVER_FAILED"
    };
    AppError::external(code, error.to_string())
}

fn shutdown_runtime(runtime: tokio::runtime::Runtime, grace: Duration) {
    runtime.shutdown_timeout(grace);
}

fn record_mcp_system_error(
    logger: Option<&EventLogger>,
    component: &str,
    error: AppError,
) -> AppError {
    record_app_system_error(logger, component, &error, serde_json::json!({}));
    error
}

fn mark_managed_failure(runner: &Option<Arc<ManagedRunner>>, kind: ExitKind, error: &AppError) {
    if let Some(runner) = runner {
        let _ = runner.mark_failed(kind, error.exit_code(), error.code());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        sync::mpsc,
        time::{Duration, Instant},
    };

    use ah_runtime::PluginManager;

    use super::{
        is_updater_mcp_restore_fast_path, is_version_fast_path, map_mcp_transport_error,
        resolve_invocation_command, shutdown_runtime,
    };

    #[test]
    fn mcp_transport_errors_use_stable_diagnostic_codes() {
        let timeout =
            map_mcp_transport_error(ah_mcp::McpAdapterError::ShutdownTimeout { grace_ms: 5_000 });
        assert_eq!(timeout.code(), "MCP_SHUTDOWN_TIMEOUT");
        assert_eq!(
            timeout.detail_message(),
            "MCP shutdown exceeded the configured 5000 ms grace period"
        );

        let service = map_mcp_transport_error(ah_mcp::McpAdapterError::Service(
            "transport failed".to_owned(),
        ));
        assert_eq!(service.code(), "MCP_SERVER_FAILED");
    }

    #[test]
    fn runtime_shutdown_respects_grace_for_blocking_handlers() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        runtime.spawn_blocking(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let started = Instant::now();
        shutdown_runtime(runtime, Duration::from_millis(10));
        assert!(started.elapsed() < Duration::from_secs(1));

        release_tx.send(()).unwrap();
    }

    #[test]
    fn version_fast_path_accepts_only_standalone_version_flags() {
        assert!(is_version_fast_path(&[
            OsString::from("ah"),
            OsString::from("--version"),
        ]));
        assert!(is_version_fast_path(&[
            OsString::from("ah"),
            OsString::from("-V"),
        ]));
        assert!(!is_version_fast_path(&[
            OsString::from("ah"),
            OsString::from("--quiet"),
            OsString::from("--version"),
        ]));
        assert!(!is_version_fast_path(&[
            OsString::from("ah"),
            OsString::from("file"),
            OsString::from("--version"),
        ]));
    }

    #[test]
    fn updater_mcp_restore_fast_path_allows_reconcile_install() {
        unsafe { std::env::set_var("AH_UPDATER_MCP_RESTORE", "1") };
        let allowed = is_updater_mcp_restore_fast_path(
            &["ah", "--json", "mcp", "service", "install"].map(OsString::from),
        );
        unsafe { std::env::remove_var("AH_UPDATER_MCP_RESTORE") };

        assert!(allowed);
    }

    #[test]
    fn recovery_is_routed_before_version_config_and_plugin_discovery() {
        let source = include_str!("runtime_flow.rs");
        let recovery = source.find("recover_before_startup(").unwrap();
        let version = source.find("if is_version_fast_path").unwrap();
        let config = source.find("ConfigContext::load()").unwrap();
        let discovery = source.find("let mut load_report = discovery(").unwrap();
        assert!(recovery < version);
        assert!(recovery < config);
        assert!(recovery < discovery);
    }

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
