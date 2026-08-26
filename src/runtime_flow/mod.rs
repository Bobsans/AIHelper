//! One run of the process, in four phases: parse, bootstrap, dispatch, report.
//!
//! The phases are what this file is for, and in 884 production lines they were
//! hard to see: the 164-line `mcp serve` loop, the credential extraction and
//! the event-log recording sat between them.
//!
//! | Module      | Owns                                                     |
//! |-------------|----------------------------------------------------------|
//! | `invoke`    | running the parsed command, credentials, diagnostics      |
//! | `mcp_serve` | the one command that owns the process for its lifetime    |
//! | `record`    | what the run tells the event log and prints about plugins |

use std::{
    collections::{BTreeMap, btree_map::Entry},
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

use ah_plugin_api::CommandCatalog;
use ah_runtime::{
    InvocationOutcome, PluginLoadReport, PluginManager, PluginSource, SecretResolver,
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
        lifecycle::ManagedMcpGuard,
        model::ExitKind,
        runner::{ManagedPreflight, ManagedRunner},
    },
    output::{emit_muted_stderr, emit_warning},
    plugin_settings::PluginSettings,
    plugins,
};

mod invoke;
mod mcp_serve;
mod record;
use invoke::{command_log_name, execution, plugin_source_name};
use mcp_serve::{McpServeConfig, execute_mcp_serve};
use record::{
    record_app_system_error, record_discovery_events, render_discovery_diagnostics,
    successful_exit_command_name,
};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use invoke::{extract_credential_args, resolve_invocation_command};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use mcp_serve::{map_mcp_transport_error, shutdown_runtime};

/// The two ports the updater asks for, wired to this process's answers.
fn updater_host() -> crate::updater::Host<'static> {
    crate::updater::Host {
        service: &ManagedMcpGuard,
        smoke: &crate::upgrade::BoundedSmokeRunner,
    }
}

/// What a phase decides: either the process is done, or it may continue.
enum Step<T> {
    Done(Result<(), AppError>),
    Continue(T),
}

pub(crate) fn run() -> Result<(), AppError> {
    let started = Instant::now();
    let raw_args = std::env::args_os().collect::<Vec<_>>();
    let entry = crate::entry::detect(&raw_args)?;

    match answer_before_startup(&entry)? {
        Step::Done(outcome) => return outcome,
        Step::Continue(()) => {}
    }

    let session = Session::open(&raw_args)?;
    let managed_runner = match route_without_plugins(&raw_args, entry.route)? {
        Step::Done(outcome) => return outcome,
        Step::Continue(runner) => runner,
    };

    dispatch(raw_args, session, managed_runner, started)
}

/// The answers that need nothing built: the updater's smoke handoff, crash
/// recovery, and a bare `--version`.
///
/// Recovery runs here, before the plugin-aware parse, because it must not
/// depend on the command being valid. A handoff skips it: the updater is
/// already mid-transaction and a second recovery would fight it.
fn answer_before_startup(entry: &crate::entry::Startup) -> Result<Step<()>, AppError> {
    if entry.handoff == Some(crate::entry::Handoff::InstalledSmoke) {
        println!("ah {}", env!("CARGO_PKG_VERSION"));
        return Ok(Step::Done(Ok(())));
    }
    if entry.handoff.is_none()
        && matches!(
            crate::updater::recovery::recover_before_startup(
                matches!(entry.route, crate::entry::Route::ManagedServe),
                &ManagedMcpGuard,
            )?,
            crate::updater::recovery::EarlyRecoveryOutcome::RecoveryLaunched
        )
    {
        emit_warning("update recovery started; rerun the command after recovery completes");
        return Ok(Step::Done(Ok(())));
    }
    if entry.version_only {
        println!("ah {}", env!("CARGO_PKG_VERSION"));
        return Ok(Step::Done(Ok(())));
    }
    Ok(Step::Continue(()))
}

/// The redacted argv and the logger, established once so every later failure
/// reports the same way.
struct Session {
    logged_argv: Vec<String>,
    logger: Option<Arc<EventLogger>>,
}

impl Session {
    /// # Errors
    ///
    /// [`AppError`] when `--cwd` names a directory the process cannot enter.
    /// The failure is recorded before it is returned, because nothing later in
    /// the run will get the chance.
    fn open(raw_args: &[OsString]) -> Result<Self, AppError> {
        let logged_raw_args = cli::redact_secret_command_argv(raw_args);
        let logged_argv = logged_raw_args
            .iter()
            .skip(1)
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // Resolved before the logger, because a relative `AH_CONFIG_DIR` is
        // taken relative to this directory and the logger is the first thing
        // that opens it.
        match cli::initial_cwd_from_raw_args(raw_args) {
            Ok(Some(cwd)) => crate::config::set_base_dir(cwd),
            Ok(None) => {}
            Err(error) => {
                let logger = EventLogger::new();
                record_app_system_error(
                    logger.as_ref(),
                    "startup",
                    &error,
                    serde_json::json!({"argv": logged_argv}),
                );
                return Err(error);
            }
        }
        Ok(Self {
            logged_argv,
            logger: EventLogger::new().map(Arc::new),
        })
    }

    fn logger(&self) -> Option<&EventLogger> {
        self.logger.as_deref()
    }

    fn fail(&self, stage: &str, error: &AppError) {
        record_app_system_error(
            self.logger(),
            stage,
            error,
            serde_json::json!({"argv": self.logged_argv}),
        );
    }
}

/// The entry points answered before plugin discovery: `upgrade`, the managed
/// service commands, and the preflight for a managed `mcp serve`.
///
/// They are separate parses from the main one because the full CLI cannot be
/// the first parse - its shape depends on which plugins loaded, and discovery
/// is what these avoid. They are no longer separate *scans*: this used to ask
/// each parser in turn whether the command was its own, so every invocation
/// walked argv once per candidate route before the real parse walked it again.
/// [`crate::entry::Route`] answers that once, and at most one parse follows.
fn route_without_plugins(
    raw_args: &[OsString],
    route: crate::entry::Route,
) -> Result<Step<Option<Arc<ManagedRunner>>>, AppError> {
    match route {
        crate::entry::Route::Full => Ok(Step::Continue(None)),
        crate::entry::Route::Upgrade => match crate::updater::command::parse(raw_args)? {
            crate::updater::command::EarlyUpgradeRoute::ExitSuccess => Ok(Step::Done(Ok(()))),
            crate::updater::command::EarlyUpgradeRoute::Execute { request, options } => Ok(
                Step::Done(crate::updater::execute(request, options, &updater_host())),
            ),
        },
        crate::entry::Route::Service | crate::entry::Route::ManagedServe => {
            match crate::mcp_service::command::parse(raw_args)? {
                EarlyRoute::ExitSuccess => Ok(Step::Done(Ok(()))),
                EarlyRoute::Service(command) => {
                    Ok(Step::Done(crate::mcp_service::lifecycle::execute(command)))
                }
                EarlyRoute::ManagedServe { definition_path } => {
                    match ManagedRunner::preflight(&definition_path)? {
                        ManagedPreflight::AlreadyRunning => Ok(Step::Done(Ok(()))),
                        ManagedPreflight::Ready(runner) => {
                            Ok(Step::Continue(Some(Arc::new(*runner))))
                        }
                    }
                }
            }
        }
    }
}

/// Build what the command needs, parse it against the loaded catalog, run it,
/// and record the outcome.
fn dispatch(
    raw_args: Vec<OsString>,
    session: Session,
    managed_runner: Option<Arc<ManagedRunner>>,
    started: Instant,
) -> Result<(), AppError> {
    let mut runtime = match startup(raw_args, session.logger(), &session.logged_argv) {
        Ok(runtime) => runtime,
        Err(error) => {
            mark_managed_failure(&managed_runner, ExitKind::StartupFailure, &error);
            return Err(error);
        }
    };
    let mut load_report = discovery(&runtime.config, &mut runtime.manager, &runtime.settings);
    record_discovery_events(session.logger(), &load_report);

    let command = match routing(runtime.raw_args, &runtime.manager) {
        Ok(RoutingOutcome::ExitSuccess) => {
            if let Some(logger) = &session.logger {
                logger.record_cli_command(
                    successful_exit_command_name(&session.logged_argv),
                    session.logged_argv.clone(),
                    started.elapsed(),
                    None,
                );
            }
            return Ok(());
        }
        Ok(RoutingOutcome::Command(command)) => command,
        Err(error) => {
            session.fail("cli_parse", &error);
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
        runtime.vault,
        runtime.settings,
        session.logger.clone(),
        managed_runner,
    );
    if let Some(logger) = &session.logger {
        logger.record_cli_command_with_outcome(
            &command_name,
            session.logged_argv.clone(),
            started.elapsed(),
            result.as_ref().ok().and_then(|outcome| outcome.as_ref()),
            result.as_ref().err(),
        );
    }
    result.map(|_| ())
}

struct RuntimeStartup {
    raw_args: Vec<OsString>,
    config: ConfigContext,
    settings: PluginSettings,
    manager: PluginManager,
    vault: Arc<crate::secrets::VaultStore>,
}

enum RoutingOutcome {
    ExitSuccess,
    Command(RuntimeCommand),
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
    let vault = crate::runtime_vault(&config);
    let mut manager = PluginManager::new();
    let resolver: Arc<dyn SecretResolver> = vault.clone();
    manager.set_secret_resolver(resolver);
    manager.reserve_dynamic_domains(["ai", "plugins", "mcp", "secrets", "upgrade"]);
    for plugin in plugins::builtins() {
        manager.register_builtin(plugin);
    }

    Ok(RuntimeStartup {
        raw_args,
        config,
        settings,
        manager,
        vault,
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

fn mark_managed_failure(runner: &Option<Arc<ManagedRunner>>, kind: ExitKind, error: &AppError) {
    if let Some(runner) = runner {
        let _ = runner.mark_failed(kind, error.exit_code(), error.code());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    use ah_runtime::PluginManager;

    use super::{
        Step, answer_before_startup, extract_credential_args, map_mcp_transport_error,
        resolve_invocation_command, shutdown_runtime,
    };

    /// The updater's smoke handoff answers before crash recovery is even
    /// consulted, because the updater is mid-transaction and a second recovery
    /// would fight it.
    ///
    /// The ordering was previously stated only by the position of two `if`
    /// blocks inside a hundred-line function, where nothing could assert it.
    /// Splitting `run()` into phases made it something a test can call.
    #[test]
    fn the_smoke_handoff_answers_before_recovery_is_consulted() {
        let entry = crate::entry::Startup {
            version_only: true,
            handoff: Some(crate::entry::Handoff::InstalledSmoke),
            route: crate::entry::Route::Full,
        };

        let step = answer_before_startup(&entry).expect("the handoff answers");

        assert!(matches!(step, Step::Done(Ok(()))));
    }

    /// A handoff that is not the smoke one still skips recovery, and still has
    /// to let the command it names run.
    #[test]
    fn the_restore_handoff_continues_without_recovery() {
        let entry = crate::entry::Startup {
            version_only: false,
            handoff: Some(crate::entry::Handoff::ManagedRestore),
            route: crate::entry::Route::Full,
        };

        let step = answer_before_startup(&entry).expect("the handoff continues");

        assert!(matches!(step, Step::Continue(())));
    }

    #[test]
    fn credential_extraction_stops_at_the_positional_separator() {
        let argv = ["run", "--credential", "basic=api", "--", "--credential=a=b"]
            .map(str::to_owned)
            .to_vec();

        let extracted = extract_credential_args(argv).unwrap();

        assert_eq!(extracted.argv, ["run", "--", "--credential=a=b"]);
        assert_eq!(extracted.credentials["basic"], "api");
        assert_eq!(extracted.credentials.len(), 1);
    }

    #[test]
    fn credential_extraction_keeps_attached_option_values_intact() {
        let argv = ["post", "--body=--credential=a=b"]
            .map(str::to_owned)
            .to_vec();

        let extracted = extract_credential_args(argv).unwrap();

        assert_eq!(extracted.argv, ["post", "--body=--credential=a=b"]);
        assert!(extracted.credentials.is_empty());
    }

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
