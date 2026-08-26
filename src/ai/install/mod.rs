//! `ah ai install`, `ai uninstall` and `ai status`: registering AIHelper as an
//! MCP server with each supported agent, and reading back what is registered.
//!
//! This file is the request types and the dispatch. The 1304 production lines
//! it used to hold put the three commands, their report shapes and seven
//! agents' worth of configuration-path trivia in one place:
//!
//! | Module    | Owns                                                        |
//! |-----------|-------------------------------------------------------------|
//! | `report`  | what each command reports, in text and JSON shape            |
//! | `spec`    | what is being installed, where, and against which scope      |
//! | `apply`   | the mutations, and undoing them                             |
//! | `inspect` | reading state back without changing it                      |
//! | `paths`   | where each agent keeps its configuration                    |

use std::{io::IsTerminal, path::PathBuf};

use ah_runtime::PluginManager;
use serde::Serialize;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

use super::{
    json_config,
    managed::{self, ManagedAction},
    opencode_config, prompt, registrar,
    rules::{self, BlockAction as Action},
    targets::{
        self, LEGACY_SERVER_NAMES, Registrar, SERVER_NAME, Scope, ServerSpec, Target, Transport,
    },
};

use super::{
    output::{emit, emit_status},
    progress::emit_live_status,
};

mod apply;
mod inspect;
mod paths;
mod report;
mod spec;
pub(super) use apply::{install, legacy_registration, uninstall};
pub(super) use inspect::{status, status_scopes, status_target};
pub(super) use paths::{
    codex_config_path, codex_mcp_entry, copilot_user_mcp_path, system_config_directory,
    system_mcp_paths,
};
pub(super) use report::{
    ManagedReport, McpReport, RulesReport, SCHEMA_VERSION, ScopeStatus, ScopedMcpReport,
    ScopedRulesReport, StatusAction, StatusProgress, StatusReport, TargetReport, TargetStatus,
};
pub use spec::DEFAULT_HTTP_URL;
pub(super) use spec::{
    config_path_for, http_spec, project_root, registrar_label, resolve_scope, rules_block,
    status_project_root, stdio_spec,
};

#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub target: String,
    pub scope: Option<Scope>,
    pub transport: Transport,
    /// `--transport managed`: HTTP against an endpoint AIHelper provisions.
    pub managed: bool,
    pub url: Option<String>,
    pub with_mcp: bool,
    pub with_rules: bool,
    pub dry_run: bool,
    /// Ask before deciding anything. Set only for a terminal invocation that
    /// carried no decision flags.
    pub interactive: bool,
    pub assume_yes: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallRequest {
    pub target: String,
    pub scope: Option<Scope>,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct StatusRequest {
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AiCommand {
    Install(InstallRequest),
    Uninstall(UninstallRequest),
    Status(StatusRequest),
}

pub fn execute(
    manager: &PluginManager,
    command: AiCommand,
    options: GlobalOptions,
) -> Result<(), AppError> {
    let cwd = options.cwd.clone();
    match command {
        AiCommand::Install(request) => {
            let report = install(manager, request, cwd.as_deref())?;
            emit(&report, &options)
        }
        AiCommand::Uninstall(request) => {
            let report = uninstall(request, cwd.as_deref())?;
            emit(&report, &options)
        }
        AiCommand::Status(request) => {
            if !options.quiet
                && matches!(options.output, OutputMode::Text)
                && std::io::stdout().is_terminal()
            {
                emit_live_status(request, cwd.as_deref())
            } else {
                let report = status(request, cwd.as_deref())?;
                emit_status(&report, &options)
            }
        }
    }
}

pub(super) fn parallel_map<T: Sync, R: Send>(
    values: &[T],
    work: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    std::thread::scope(|scope| {
        let work = &work;
        values
            .iter()
            .map(|value| scope.spawn(move || work(value)))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|worker| worker.join().expect("status worker panicked"))
            .collect()
    })
}

pub(super) fn cli_available(program: &str) -> bool {
    registrar::available(program)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::{DEFAULT_HTTP_URL, StatusProgress, http_spec, parallel_map, status_target};
    use crate::ai::targets::{Scope, ServerSpec};

    #[test]
    fn http_spec_defaults_to_the_managed_loopback_endpoint() {
        assert_eq!(
            http_spec(None).expect("default url is loopback"),
            ServerSpec::Http {
                url: DEFAULT_HTTP_URL.to_owned()
            }
        );
    }

    #[test]
    fn loopback_hosts_are_accepted_with_and_without_a_port() {
        for url in [
            "http://127.0.0.1:9000/mcp",
            "http://localhost/mcp",
            "http://[::1]:8787/mcp",
        ] {
            http_spec(Some(url)).unwrap_or_else(|_| panic!("{url} should be accepted"));
        }
    }

    #[test]
    fn non_loopback_hosts_are_refused() {
        let error = http_spec(Some("http://10.0.0.5:8787/mcp")).expect_err("must fail");
        assert_eq!(error.code(), "AI_URL_NOT_LOOPBACK");
    }

    #[test]
    fn invalid_mcp_endpoint_shapes_are_refused() {
        for url in [
            "ftp://localhost:8787/mcp",
            "http://user@localhost:8787/mcp",
            "http://localhost:not-a-port/mcp",
            "http://localhost:8787/other",
            "http://localhost:8787/mcp?token=value",
        ] {
            assert!(http_spec(Some(url)).is_err(), "{url} should be refused");
        }
    }

    #[test]
    fn parallel_map_runs_work_concurrently_and_preserves_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let values = [3, 1, 2];

        let results = parallel_map(&values, |value| {
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            active.fetch_sub(1, Ordering::SeqCst);
            value * 2
        });

        assert!(maximum.load(Ordering::SeqCst) > 1);
        assert_eq!(results, vec![6, 2, 4]);
    }

    #[test]
    fn status_target_reports_cli_and_rules_before_mcp() {
        let root = tempfile::tempdir().unwrap();
        let mut events = Vec::new();

        status_target(
            crate::ai::targets::find("cursor").unwrap(),
            root.path(),
            true,
            |event| {
                events.push(match event {
                    StatusProgress::Cli { .. } => "cli",
                    StatusProgress::Rules { scope, .. } => match scope {
                        Scope::User => "rules:user",
                        Scope::Project => "rules:project",
                        _ => "rules:other",
                    },
                    StatusProgress::Mcp { scope, .. } => match scope {
                        Scope::User => "mcp:user",
                        Scope::Project => "mcp:project",
                        _ => "mcp:other",
                    },
                })
            },
        )
        .unwrap();

        assert_eq!(
            events,
            vec![
                "cli",
                "rules:user",
                "mcp:user",
                "rules:project",
                "mcp:project"
            ]
        );
    }
}
