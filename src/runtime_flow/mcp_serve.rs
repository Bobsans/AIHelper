//! `ah mcp serve`, which is the one command that owns the process for its
//! whole lifetime rather than returning a result.

use super::*;

pub(super) const MCP_RUNTIME_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub(super) struct McpServeConfig {
    pub(super) manager: PluginManager,
    pub(super) vault: Arc<crate::secrets::VaultStore>,
    pub(super) settings: PluginSettings,
    pub(super) transport: cli::McpTransport,
    pub(super) port: u16,
    pub(super) max_active: usize,
    pub(super) default_timeout_ms: u64,
    pub(super) options: cli::GlobalOptions,
    pub(super) logger: Option<Arc<EventLogger>>,
    pub(super) managed_runner: Option<Arc<ManagedRunner>>,
}

pub(super) fn execute_mcp_serve(config: McpServeConfig) -> Result<(), AppError> {
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
            // What the preflight used to apply with a process-wide `chdir`.
            managed_options.cwd = Some(definition.working_directory.clone());
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
    // The directory a request that names none is served from. A request may
    // still carry its own `context.cwd`; this is only the default.
    let cwd = match options.cwd.clone() {
        Some(cwd) => Ok(cwd),
        None => std::env::current_dir()
            .map_err(|source| AppError::cwd(std::path::PathBuf::from("."), source))
            .map_err(|error| {
                record_mcp_system_error(config.logger.as_deref(), "mcp_server", error)
            }),
    };
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
            let settings = Arc::new(std::sync::Mutex::new(config.settings));
            let manager = Arc::new_cyclic(|weak| {
                let mut manager = config.manager;
                for plugin in crate::host_commands::builtins(
                    weak.clone(),
                    Arc::clone(&settings),
                    Arc::clone(&config.vault),
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
            let secret_setup: Arc<dyn ah_mcp::SecretSetupService> = Arc::new(
                crate::secrets::VaultSetupService::new(Arc::clone(&config.vault)),
            );
            server = server.with_secret_setup(secret_setup);
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

pub(super) fn map_mcp_transport_error(error: ah_mcp::McpAdapterError) -> AppError {
    let code = if matches!(&error, ah_mcp::McpAdapterError::ShutdownTimeout { .. }) {
        "MCP_SHUTDOWN_TIMEOUT"
    } else {
        "MCP_SERVER_FAILED"
    };
    AppError::external(code, error.to_string())
}

pub(super) fn shutdown_runtime(runtime: tokio::runtime::Runtime, grace: Duration) {
    runtime.shutdown_timeout(grace);
}

pub(super) fn record_mcp_system_error(
    logger: Option<&EventLogger>,
    component: &str,
    error: AppError,
) -> AppError {
    record_app_system_error(logger, component, &error, serde_json::json!({}));
    error
}
