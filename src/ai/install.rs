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

pub const DEFAULT_HTTP_URL: &str = "http://127.0.0.1:8787/mcp";

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

#[derive(Debug, Clone, Serialize)]
pub(super) struct McpReport {
    pub(super) action: Action,
    pub(super) registrar: &'static str,
    pub(super) scope: Scope,
    pub(super) transport: Option<Transport>,
    pub(super) url: Option<String>,
    pub(super) path: Option<String>,
    pub(super) commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct RulesReport {
    pub(super) action: Action,
    pub(super) path: String,
}

#[derive(Debug, Serialize)]
pub(super) struct ManagedReport {
    pub(super) action: ManagedAction,
    pub(super) endpoint: String,
}

#[derive(Debug, Serialize)]
pub(super) struct TargetReport {
    pub(super) command: &'static str,
    pub(super) schema_version: u32,
    pub(super) target: &'static str,
    pub(super) scope: Scope,
    pub(super) changed: bool,
    pub(super) dry_run: bool,
    pub(super) mcp: McpReport,
    pub(super) rules: RulesReport,
    pub(super) managed_service: Option<ManagedReport>,
    pub(super) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct StatusReport {
    pub(super) command: &'static str,
    pub(super) schema_version: u32,
    pub(super) targets: Vec<TargetStatus>,
}

#[derive(Debug, Serialize)]
pub(super) struct TargetStatus {
    pub(super) target: &'static str,
    pub(super) scope: Scope,
    pub(super) cli: Option<&'static str>,
    pub(super) cli_available: bool,
    /// A registration left by an older AIHelper under its previous server name.
    pub(super) legacy_server: Option<&'static str>,
    pub(super) mcp: McpReport,
    pub(super) rules: RulesReport,
    pub(super) scopes: Vec<ScopeStatus>,
}

#[derive(Debug, Serialize)]
pub(super) struct ScopeStatus {
    pub(super) scope: Scope,
    pub(super) mcp: Option<ScopedMcpReport>,
    pub(super) rules: Option<ScopedRulesReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ScopedMcpReport {
    pub(super) action: StatusAction,
    pub(super) registrar: &'static str,
    pub(super) scope: Scope,
    pub(super) transport: Option<Transport>,
    pub(super) url: Option<String>,
    pub(super) path: Option<String>,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ScopedRulesReport {
    pub(super) action: StatusAction,
    pub(super) path: Option<String>,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StatusAction {
    Installed,
    NotPresent,
    Unknown,
}

pub(super) enum StatusProgress {
    Cli {
        cli: Option<&'static str>,
        available: bool,
    },
    Mcp {
        scope: Scope,
        report: ScopedMcpReport,
    },
    Rules {
        scope: Scope,
        report: ScopedRulesReport,
    },
}

pub(super) const SCHEMA_VERSION: u32 = 1;

pub fn execute(
    manager: &PluginManager,
    command: AiCommand,
    options: GlobalOptions,
) -> Result<(), AppError> {
    match command {
        AiCommand::Install(request) => {
            let report = install(manager, request)?;
            emit(&report, options)
        }
        AiCommand::Uninstall(request) => {
            let report = uninstall(request)?;
            emit(&report, options)
        }
        AiCommand::Status(request) => {
            if !options.quiet
                && matches!(options.output, OutputMode::Text)
                && std::io::stdout().is_terminal()
            {
                emit_live_status(request)
            } else {
                let report = status(request)?;
                emit_status(&report, options)
            }
        }
    }
}

pub(super) fn project_root() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))
}

pub(super) fn status_project_root(cwd: &std::path::Path) -> Option<PathBuf> {
    if let Some(root) = cwd.ancestors().find(|path| path.join(".git").exists()) {
        return Some(root.to_path_buf());
    }
    const MARKERS: [&str; 18] = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "opencode.json",
        "opencode.jsonc",
        ".mcp.json",
        ".claude",
        ".codex",
        ".gemini",
        ".cursor",
        ".vscode",
        ".github",
        ".opencode",
    ];
    MARKERS
        .iter()
        .any(|marker| cwd.join(marker).exists())
        .then(|| cwd.to_path_buf())
}

pub(super) fn resolve_scope(target: &Target, requested: Option<Scope>) -> Result<Scope, AppError> {
    let scope = requested.unwrap_or(target.default_scope);
    target.require_scope(scope)?;
    Ok(scope)
}

pub(super) fn stdio_spec() -> Result<ServerSpec, AppError> {
    let executable = std::env::current_exe().map_err(|source| {
        AppError::external(
            "AI_EXECUTABLE_UNRESOLVED",
            format!("unable to resolve the AIHelper executable path: {source}"),
        )
    })?;
    Ok(ServerSpec::Stdio {
        command: executable.display().to_string(),
        args: vec!["mcp".to_owned(), "serve".to_owned()],
    })
}

/// The server has no authentication or TLS, so only loopback endpoints are
/// ever written into an agent configuration.
pub(super) fn http_spec(url: Option<&str>) -> Result<ServerSpec, AppError> {
    let url = url.unwrap_or(DEFAULT_HTTP_URL).to_owned();
    let parsed = reqwest::Url::parse(&url).map_err(|source| {
        AppError::external("AI_URL_INVALID", format!("invalid MCP URL {url}: {source}"))
    })?;
    if parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/mcp"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AppError::external(
            "AI_URL_INVALID",
            format!("{url} must be a plain HTTP loopback URL ending exactly in /mcp"),
        ));
    }
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if !matches!(host, "127.0.0.1" | "::1" | "localhost") {
        return Err(AppError::external(
            "AI_URL_NOT_LOOPBACK",
            format!(
                "{url} is not a loopback endpoint; the AIHelper MCP server has no \
                 authentication or TLS and must not be exposed off-host"
            ),
        ));
    }
    Ok(ServerSpec::Http { url })
}

pub(super) fn rules_block(manager: &PluginManager) -> String {
    let mut domains = manager
        .collect_plugin_manuals()
        .into_iter()
        .map(|manual| manual.domain)
        .collect::<Vec<_>>();
    domains.sort();
    domains.dedup();
    rules::render_block(&domains)
}

pub(super) fn install(
    manager: &PluginManager,
    mut request: InstallRequest,
) -> Result<TargetReport, AppError> {
    let target = targets::find(&request.target)?;
    if request.interactive {
        let answers = prompt::ask(target, DEFAULT_HTTP_URL)?;
        request.scope = Some(answers.scope);
        request.with_mcp = answers.with_mcp;
        request.with_rules = answers.with_rules;
        request.transport = answers.transport;
        request.managed = answers.managed;
        request.url = answers.url;
    }
    if !request.with_mcp && !request.with_rules {
        return Err(AppError::external(
            "AI_NOTHING_SELECTED",
            "--mcp-only and --rules-only cannot both be disabled; nothing would be installed",
        ));
    }
    if request.url.is_some() {
        if request.managed {
            return Err(AppError::invalid_argument(
                "--url cannot be combined with --transport managed; \
                 the managed service owns its own endpoint",
            ));
        }
        if request.transport != Transport::Http {
            return Err(AppError::invalid_argument(
                "--url can be used only with --transport http",
            ));
        }
    }
    let scope = resolve_scope(target, request.scope)?;
    let root = project_root()?;
    let mut warnings = Vec::new();

    // Everything up to the confirmation is read-only: classification, path
    // resolution, and the readiness probe. Nothing is provisioned or written.
    let mut managed_snapshot = None;
    let planned_spec = if request.with_mcp {
        Some(if request.managed {
            let snapshot = plan_managed()?;
            let endpoint = snapshot
                .endpoint
                .clone()
                .unwrap_or_else(|| DEFAULT_HTTP_URL.to_owned());
            managed_snapshot = Some(snapshot);
            ServerSpec::Http { url: endpoint }
        } else {
            match request.transport {
                Transport::Stdio => stdio_spec()?,
                Transport::Http => {
                    let spec = http_spec(request.url.as_deref())?;
                    if let Some(warning) = probe_readiness(&spec) {
                        warnings.push(warning);
                    }
                    spec
                }
            }
        })
    } else {
        None
    };

    let rules_path = target.rules_path(scope, &root)?.display().to_string();
    if request.interactive && !request.assume_yes {
        let summary = plan_summary(
            target,
            scope,
            planned_spec.as_ref(),
            managed_snapshot.as_ref(),
            request.with_rules.then_some(rules_path.as_str()),
        );
        if !prompt::confirm(&summary)? {
            return Ok(cancelled_report(target, scope, rules_path));
        }
    }

    let mut managed_service = None;
    let mcp = match planned_spec {
        Some(spec) => {
            let spec = match managed_snapshot {
                Some(snapshot) if !request.dry_run => {
                    let (endpoint, action) = managed::ensure_ready(&snapshot)?;
                    managed_service = Some(ManagedReport {
                        action,
                        endpoint: endpoint.clone(),
                    });
                    ServerSpec::Http { url: endpoint }
                }
                Some(snapshot) => {
                    managed_service = Some(ManagedReport {
                        action: managed::planned_action(&snapshot),
                        endpoint: spec.url().unwrap_or(DEFAULT_HTTP_URL).to_owned(),
                    });
                    spec
                }
                None => spec,
            };
            install_mcp(target, scope, &spec, request.dry_run, &root, &mut warnings)?
        }
        None => skipped_mcp(target, scope),
    };

    let rules_report = if request.with_rules {
        install_rules(target, scope, &root, &rules_block(manager), request.dry_run)?
    } else {
        RulesReport {
            action: Action::Skipped,
            path: rules_path,
        }
    };

    Ok(TargetReport {
        command: "ai.install",
        schema_version: SCHEMA_VERSION,
        target: target.name,
        scope,
        changed: mcp.action.changed() || rules_report.action.changed(),
        dry_run: request.dry_run,
        mcp,
        rules: rules_report,
        managed_service,
        warnings,
    })
}

/// Classify the managed service without provisioning it. Refuses an
/// unsupported platform and a drifted registration before anything happens.
pub(super) fn plan_managed() -> Result<managed::Snapshot, AppError> {
    let snapshot = managed::detect()?;
    managed::require_usable(&snapshot)?;
    Ok(snapshot)
}

pub(super) fn plan_summary(
    target: &Target,
    scope: Scope,
    spec: Option<&ServerSpec>,
    managed_snapshot: Option<&managed::Snapshot>,
    rules_path: Option<&str>,
) -> String {
    let mut lines = vec![format!(
        "About to configure {} in the {} scope:",
        target.name,
        scope.as_str()
    )];
    match spec {
        Some(spec) => {
            let invocation =
                registrar::add_invocation(target, target.mcp_scope(scope), SERVER_NAME, spec);
            lines.push(format!("  run   {}", invocation.render()));
            if let Some(snapshot) = managed_snapshot {
                lines.push(format!(
                    "  managed service: {}",
                    managed::planned_action(snapshot).as_str()
                ));
            }
        }
        None => lines.push("  MCP server: skipped".to_owned()),
    }
    match rules_path {
        Some(path) => lines.push(format!("  write {path}")),
        None => lines.push("  rules block: skipped".to_owned()),
    }
    lines.join("\n")
}

pub(super) fn cancelled_report(target: &Target, scope: Scope, rules_path: String) -> TargetReport {
    TargetReport {
        command: "ai.install",
        schema_version: SCHEMA_VERSION,
        target: target.name,
        scope,
        changed: false,
        dry_run: false,
        mcp: skipped_mcp(target, scope),
        rules: RulesReport {
            action: Action::Skipped,
            path: rules_path,
        },
        managed_service: None,
        warnings: vec!["cancelled at the confirmation prompt; nothing was changed".to_owned()],
    }
}

/// A refused connection is the agent's problem later, so warn rather than fail.
pub(super) fn probe_readiness(spec: &ServerSpec) -> Option<String> {
    let url = spec.url()?;
    let readiness = url
        .strip_suffix("/mcp")
        .map(|origin| format!("{origin}/health/ready"))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .ok()?;
    match client.get(&readiness).send() {
        Ok(response) if response.status().is_success() => None,
        _ => Some(format!(
            "no MCP server answered {readiness}; the agent will see a refused connection \
             until one is started (`ah mcp serve --transport http`)"
        )),
    }
}

pub(super) fn registrar_label(target: &Target) -> &'static str {
    match target.registrar {
        Registrar::Cli { .. } => "cli",
        Registrar::File => "file",
    }
}

pub(super) fn skipped_mcp(target: &Target, scope: Scope) -> McpReport {
    McpReport {
        action: Action::Skipped,
        registrar: registrar_label(target),
        scope: target.mcp_scope(scope),
        transport: None,
        url: None,
        path: None,
        commands: Vec::new(),
    }
}

pub(super) fn config_path_for(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
) -> Result<Option<String>, AppError> {
    if target.probe == targets::ProbeKind::OpenCode {
        return Ok(Some(
            opencode_config::path(scope, root)?.display().to_string(),
        ));
    }
    match (target.registrar, target.json) {
        (Registrar::File, Some(config)) => {
            Ok(Some(config.path(scope, root)?.display().to_string()))
        }
        _ => Ok(None),
    }
}

/// A registration under a superseded server name, if the agent still has one.
pub(super) fn legacy_registration(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
) -> Result<Option<&'static str>, AppError> {
    for name in LEGACY_SERVER_NAMES {
        let present = if target.probe == targets::ProbeKind::OpenCode {
            opencode_config::contains(scope, root, name)?
        } else {
            registrar::probe(target, scope, name, root)?.is_some()
        };
        if present {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Remove one server name through whichever backend the target uses.
pub(super) fn apply_remove(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
    name: &str,
    dry_run: bool,
) -> Result<Vec<registrar::Invocation>, AppError> {
    match target.registrar {
        Registrar::Cli { .. } => {
            let invocation = registrar::remove_invocation(target, scope, name);
            if !dry_run {
                registrar::run(&invocation)?;
            }
            Ok(vec![invocation])
        }
        Registrar::File => {
            if target.probe == targets::ProbeKind::OpenCode {
                if !dry_run {
                    opencode_config::remove(scope, root, name)?;
                }
                return Ok(Vec::new());
            }
            let config = target.json_config()?;
            let file = config.path(scope, root)?;
            if !dry_run
                && let Some(document) =
                    json_config::remove(json_config::read(&file)?, &config, name)
            {
                json_config::write(&file, &document)?;
            }
            Ok(Vec::new())
        }
    }
}

pub(super) fn install_mcp(
    target: &Target,
    scope: Scope,
    spec: &ServerSpec,
    dry_run: bool,
    root: &std::path::Path,
    warnings: &mut Vec<String>,
) -> Result<McpReport, AppError> {
    let mcp_scope = target.mcp_scope(scope);
    if mcp_scope != scope {
        warnings.push(format!(
            "{} stores MCP servers only in the {} scope; the `{SERVER_NAME}` server lives there regardless of the requested scope",
            target.name,
            mcp_scope.as_str()
        ));
    }
    let existing = registrar::probe(target, mcp_scope, SERVER_NAME, root)?;
    let present = if target.probe == targets::ProbeKind::OpenCode {
        opencode_config::contains(mcp_scope, root, SERVER_NAME)?
    } else {
        existing.is_some()
    };
    let action = match &existing {
        Some(current) if current == spec => Action::Unchanged,
        Some(current) => {
            warnings.push(format!(
                "replacing the existing `{SERVER_NAME}` server registered for {} with transport {}",
                target.name,
                current.transport().as_str()
            ));
            Action::Updated
        }
        None if present => Action::Updated,
        None => Action::Installed,
    };

    let mut commands = Vec::new();
    let path = config_path_for(target, mcp_scope, root)?;
    if action.changed() {
        match target.registrar {
            Registrar::Cli { .. } => {
                // `add` over an existing name is not an update in every agent
                // CLI, so a replacement removes first. A JSON merge needs no
                // such step: it replaces the entry outright.
                if action == Action::Updated {
                    commands.extend(apply_remove(target, mcp_scope, root, SERVER_NAME, dry_run)?);
                }
                let invocation = registrar::add_invocation(target, mcp_scope, SERVER_NAME, spec);
                if !dry_run && let Err(error) = registrar::run(&invocation) {
                    if let Some(previous) = existing.as_ref() {
                        let rollback =
                            registrar::add_invocation(target, mcp_scope, SERVER_NAME, previous);
                        if let Err(rollback_error) = registrar::run(&rollback) {
                            return Err(AppError::external(
                                "AI_AGENT_CLI_FAILED",
                                format!(
                                    "{error}; restoring the previous registration also failed: {rollback_error}"
                                ),
                            ));
                        }
                    }
                    return Err(error);
                }
                commands.push(invocation);
            }
            Registrar::File => {
                if target.probe == targets::ProbeKind::OpenCode {
                    let file = opencode_config::path(mcp_scope, root)?;
                    if !dry_run {
                        opencode_config::merge(&file, SERVER_NAME, spec)?;
                    }
                } else {
                    let config = target.json_config()?;
                    let file = config.path(mcp_scope, root)?;
                    if !dry_run {
                        let document = json_config::merge(
                            json_config::read(&file)?,
                            &config,
                            SERVER_NAME,
                            spec,
                        );
                        json_config::write(&file, &document)?;
                    }
                }
            }
        }
    }
    // An older AIHelper registered a different server name; leaving it behind
    // would give the agent two identical servers.
    if let Some(legacy) = legacy_registration(target, mcp_scope, root)? {
        warnings.push(format!(
            "removing the legacy `{legacy}` registration left by an earlier AIHelper"
        ));
        commands.extend(apply_remove(target, mcp_scope, root, legacy, dry_run)?);
    }

    Ok(McpReport {
        action,
        registrar: registrar_label(target),
        scope: mcp_scope,
        transport: Some(spec.transport()),
        url: spec.url().map(str::to_owned),
        path,
        commands: commands.iter().map(registrar::Invocation::render).collect(),
    })
}

pub(super) fn install_rules(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
    block: &str,
    dry_run: bool,
) -> Result<RulesReport, AppError> {
    let path = target.rules_path(scope, root)?;
    let hint = path.display().to_string();
    let existing = rules::read(&path)?;
    let (contents, action) = rules::upsert(existing.as_deref(), block, &hint)?;
    if !dry_run && action.changed() {
        rules::write(&path, &contents)?;
    }
    Ok(RulesReport { action, path: hint })
}

pub(super) fn uninstall(request: UninstallRequest) -> Result<TargetReport, AppError> {
    let target = targets::find(&request.target)?;
    let scope = resolve_scope(target, request.scope)?;
    let root = project_root()?;
    let mcp_scope = target.mcp_scope(scope);
    let mut warnings = Vec::new();

    let existing = registrar::probe(target, mcp_scope, SERVER_NAME, &root)?;
    let present = if target.probe == targets::ProbeKind::OpenCode {
        opencode_config::contains(mcp_scope, &root, SERVER_NAME)?
    } else {
        existing.is_some()
    };
    let mut commands = Vec::new();
    let config_path = config_path_for(target, mcp_scope, &root)?;
    let mut mcp_action = if present {
        commands.extend(apply_remove(
            target,
            mcp_scope,
            &root,
            SERVER_NAME,
            request.dry_run,
        )?);
        Action::Removed
    } else {
        Action::NotPresent
    };
    // A registration written by an older AIHelper carries a different name and
    // would otherwise survive an uninstall.
    if let Some(legacy) = legacy_registration(target, mcp_scope, &root)? {
        warnings.push(format!(
            "also removing the legacy `{legacy}` registration left by an earlier AIHelper"
        ));
        commands.extend(apply_remove(
            target,
            mcp_scope,
            &root,
            legacy,
            request.dry_run,
        )?);
        mcp_action = Action::Removed;
    }

    let path = target.rules_path(scope, &root)?;
    let hint = path.display().to_string();
    let rules_action = match rules::read(&path)? {
        None => Action::NotPresent,
        Some(contents) => {
            let (updated, action) = rules::strip(&contents, &hint)?;
            if !request.dry_run && action.changed() {
                if updated.trim().is_empty() {
                    rules::delete(&path)?;
                } else {
                    rules::write(&path, &updated)?;
                }
            }
            action
        }
    };
    if mcp_scope != scope {
        warnings.push(format!(
            "{} stores MCP servers only in the {} scope; the `{SERVER_NAME}` server lives there regardless of the requested scope",
            target.name,
            mcp_scope.as_str()
        ));
    }

    Ok(TargetReport {
        command: "ai.uninstall",
        schema_version: SCHEMA_VERSION,
        target: target.name,
        scope,
        changed: mcp_action.changed() || rules_action.changed(),
        dry_run: request.dry_run,
        mcp: McpReport {
            action: mcp_action,
            registrar: registrar_label(target),
            scope: mcp_scope,
            transport: existing.as_ref().map(ServerSpec::transport),
            url: existing
                .as_ref()
                .and_then(ServerSpec::url)
                .map(str::to_owned),
            path: config_path,
            commands: commands.iter().map(registrar::Invocation::render).collect(),
        },
        rules: RulesReport {
            action: rules_action,
            path: hint,
        },
        managed_service: None,
        warnings,
    })
}

pub(super) fn status(request: StatusRequest) -> Result<StatusReport, AppError> {
    let selected: Vec<&Target> = match request.target {
        Some(name) => vec![targets::find(&name)?],
        None => targets::TARGETS.iter().collect(),
    };
    let cwd = project_root()?;
    let project = status_project_root(&cwd);
    let in_project = project.is_some();
    let root = project.as_deref().unwrap_or(&cwd);
    let reports = parallel_map(&selected, |target| {
        status_target(target, root, in_project, |_| {})
    })
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;
    Ok(StatusReport {
        command: "ai.status",
        schema_version: SCHEMA_VERSION,
        targets: reports,
    })
}

pub(super) fn status_target(
    target: &Target,
    root: &std::path::Path,
    in_project: bool,
    mut progress: impl FnMut(StatusProgress),
) -> Result<TargetStatus, AppError> {
    let scope = if !in_project && target.supports(Scope::User) {
        Scope::User
    } else {
        target.default_scope
    };
    let mcp_scope = target.mcp_scope(scope);
    // A file-backed target has no CLI to look for, so it is always usable.
    let available = target.cli_program().is_none_or(cli_available);
    progress(StatusProgress::Cli {
        cli: target.cli_program(),
        available,
    });
    let mut scopes = Vec::new();
    for status_scope in status_scopes(target, in_project) {
        let rules = match scoped_rules_status(target, status_scope, root) {
            Ok(rules) => rules,
            Err(error) => Some(ScopedRulesReport {
                action: StatusAction::Unknown,
                path: None,
                detail: Some(error.code().to_owned()),
            }),
        };
        if let Some(report) = &rules {
            progress(StatusProgress::Rules {
                scope: status_scope,
                report: report.clone(),
            });
        }
        let mcp = match scoped_mcp_status(target, status_scope, root, available) {
            Ok(mcp) => mcp,
            Err(error) => Some(ScopedMcpReport {
                action: StatusAction::Unknown,
                registrar: registrar_label(target),
                scope: status_scope,
                transport: None,
                url: None,
                path: None,
                detail: Some(error.code().to_owned()),
            }),
        };
        if target.name != "codex"
            && let Some(report) = &mcp
        {
            progress(StatusProgress::Mcp {
                scope: status_scope,
                report: report.clone(),
            });
        }
        scopes.push(ScopeStatus {
            scope: status_scope,
            mcp,
            rules,
        });
    }
    let scoped_rules = scopes
        .iter()
        .find(|status| status.scope == scope)
        .and_then(|status| status.rules.as_ref());
    let hint = scoped_rules
        .and_then(|rules| rules.path.clone())
        .unwrap_or_else(|| {
            target
                .rules_path(scope, root)
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        });
    let rules_action = match scoped_rules.map(|rules| rules.action) {
        Some(StatusAction::Installed) => Action::Installed,
        _ => Action::NotPresent,
    };
    let (codex_existing, codex_legacy) = if target.name == "codex" && available {
        let mut names = vec![SERVER_NAME];
        names.extend_from_slice(LEGACY_SERVER_NAMES);
        let probes = registrar::probe_codex_list(target, &names)?;
        let existing = probes.first().cloned().flatten();
        let legacy = LEGACY_SERVER_NAMES
            .iter()
            .zip(probes.iter().skip(1))
            .find_map(|(name, probe)| probe.is_some().then_some(*name));
        (existing, legacy)
    } else {
        (None, None)
    };
    if target.name == "codex" {
        for status in &scopes {
            if let Some(report) = &status.mcp {
                progress(StatusProgress::Mcp {
                    scope: status.scope,
                    report: report.clone(),
                });
            }
        }
    }
    let mcp = if target.name == "codex" {
        McpReport {
            action: if codex_existing.is_some() {
                Action::Installed
            } else {
                Action::NotPresent
            },
            registrar: registrar_label(target),
            scope: mcp_scope,
            transport: codex_existing.as_ref().map(ServerSpec::transport),
            url: codex_existing
                .as_ref()
                .and_then(ServerSpec::url)
                .map(str::to_owned),
            path: config_path_for(target, mcp_scope, root)?,
            commands: Vec::new(),
        }
    } else {
        scopes
            .iter()
            .find(|status| status.scope == mcp_scope)
            .and_then(|status| status.mcp.as_ref())
            .map(|status| McpReport {
                action: if matches!(status.action, StatusAction::Installed) {
                    Action::Installed
                } else {
                    Action::NotPresent
                },
                registrar: status.registrar,
                scope: status.scope,
                transport: status.transport,
                url: status.url.clone(),
                path: status.path.clone(),
                commands: Vec::new(),
            })
            .ok_or_else(|| {
                AppError::external(
                    "AI_STATUS_SCOPE_UNSUPPORTED",
                    format!(
                        "{} has no MCP status for scope {}",
                        target.name,
                        mcp_scope.as_str()
                    ),
                )
            })?
    };
    let legacy = if target.name == "codex" {
        codex_legacy
    } else if available {
        legacy_registration(target, mcp_scope, root)?
    } else {
        None
    };
    Ok(TargetStatus {
        target: target.name,
        scope,
        cli: target.cli_program(),
        cli_available: available,
        legacy_server: legacy,
        mcp,
        rules: RulesReport {
            action: rules_action,
            path: hint,
        },
        scopes,
    })
}

pub(super) fn status_scopes(target: &Target, in_project: bool) -> Vec<Scope> {
    [Scope::System, Scope::User, Scope::Project, Scope::Local]
        .into_iter()
        .filter(|scope| target.supports_status_mcp(*scope) || target.supports_status_rules(*scope))
        .filter(|scope| matches!(scope, Scope::System | Scope::User) || in_project)
        .collect()
}

pub(super) fn scoped_rules_status(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
) -> Result<Option<ScopedRulesReport>, AppError> {
    if !target.supports_status_rules(scope) {
        return Ok(None);
    }
    if target.name == "cursor" && scope == Scope::User {
        return Ok(Some(ScopedRulesReport {
            action: StatusAction::Unknown,
            path: None,
            detail: Some("Cursor settings".to_owned()),
        }));
    }
    let path = match (target.name, scope) {
        ("claude", Scope::System) => system_config_directory(target)?.join("CLAUDE.md"),
        ("claude", Scope::Local) => root.join("CLAUDE.local.md"),
        _ => target.rules_path(scope, root)?,
    };
    let action = match rules::read(&path)? {
        Some(contents) if contents.contains(rules::BEGIN_MARKER) => StatusAction::Installed,
        _ => StatusAction::NotPresent,
    };
    Ok(Some(ScopedRulesReport {
        action,
        path: Some(path.display().to_string()),
        detail: None,
    }))
}

pub(super) fn scoped_mcp_status(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
    cli_available: bool,
) -> Result<Option<ScopedMcpReport>, AppError> {
    if !target.supports_status_mcp(scope) {
        return Ok(None);
    }
    let (existing, path) = if target.name == "codex" {
        let path = codex_config_path(scope, root)?;
        (
            codex_mcp_entry(&path, SERVER_NAME)?,
            Some(path.display().to_string()),
        )
    } else if target.name == "copilot" && scope == Scope::User {
        let path = copilot_user_mcp_path()?;
        let config = target.json_config()?;
        let existing = json_config::read(&path)?
            .and_then(|document| json_config::lookup(&document, &config, SERVER_NAME));
        (existing, Some(path.display().to_string()))
    } else if scope == Scope::System {
        let (existing, path) = system_mcp_status(target)?;
        (existing, Some(path.display().to_string()))
    } else {
        let existing = if target.cli_program().is_none() || cli_available {
            registrar::probe(target, scope, SERVER_NAME, root)?
        } else {
            None
        };
        (existing, config_path_for(target, scope, root)?)
    };
    Ok(Some(ScopedMcpReport {
        action: if existing.is_some() {
            StatusAction::Installed
        } else {
            StatusAction::NotPresent
        },
        registrar: registrar_label(target),
        scope,
        transport: existing.as_ref().map(ServerSpec::transport),
        url: existing
            .as_ref()
            .and_then(ServerSpec::url)
            .map(str::to_owned),
        path,
        detail: None,
    }))
}

pub(super) fn system_mcp_status(
    target: &Target,
) -> Result<(Option<ServerSpec>, PathBuf), AppError> {
    let paths = system_mcp_paths(target)?;
    for path in &paths {
        let existing = if target.name == "opencode" {
            opencode_config::lookup_path(path, SERVER_NAME)?
        } else {
            let config = target.json_config().unwrap_or(targets::JsonConfig {
                key: "mcpServers",
                project_path: None,
                user_path: None,
            });
            json_config::read(path)?
                .and_then(|document| json_config::lookup(&document, &config, SERVER_NAME))
        };
        if existing.is_some() {
            return Ok((existing, path.clone()));
        }
    }
    Ok((None, paths[0].clone()))
}

pub(super) fn copilot_user_mcp_path() -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("Code").join("User").join("mcp.json"))
            .ok_or_else(|| {
                AppError::external(
                    "AI_HOME_UNRESOLVED",
                    "unable to resolve %APPDATA% for VS Code configuration",
                )
            })
    }
    #[cfg(target_os = "macos")]
    {
        Ok(targets::home_dir()?
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("mcp.json"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(targets::home_dir()?.join(".config"));
        Ok(base.join("Code").join("User").join("mcp.json"))
    }
}

pub(super) fn codex_config_path(scope: Scope, root: &std::path::Path) -> Result<PathBuf, AppError> {
    match scope {
        Scope::System => system_mcp_path(targets::find("codex")?),
        Scope::User => Ok(targets::home_dir()?.join(".codex").join("config.toml")),
        Scope::Project | Scope::Local => Ok(root.join(".codex").join("config.toml")),
    }
}

pub(super) fn codex_mcp_entry(
    path: &std::path::Path,
    name: &str,
) -> Result<Option<ServerSpec>, AppError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AppError::file_read(path.to_path_buf(), source)),
    };
    let headers = [
        format!("[mcp_servers.{name}]"),
        format!("[mcp_servers.\"{name}\"]"),
        format!("[mcp_servers.'{name}']"),
    ];
    let mut entry = false;
    for line in contents.lines().map(str::trim) {
        if line.starts_with('[') {
            if entry {
                break;
            }
            entry = headers.iter().any(|header| line == header);
            continue;
        }
        if !entry {
            continue;
        }
        if let Some(url) = toml_string(line, "url") {
            return Ok(Some(ServerSpec::Http { url }));
        }
        if let Some(command) = toml_string(line, "command") {
            return Ok(Some(ServerSpec::Stdio {
                command,
                args: Vec::new(),
            }));
        }
    }
    Ok(None)
}

pub(super) fn toml_string(line: &str, key: &str) -> Option<String> {
    let (candidate, value) = line.split_once('=')?;
    if candidate.trim() != key {
        return None;
    }
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.split_once('"').map(|(value, _)| value.to_owned()))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.split_once('\'').map(|(value, _)| value.to_owned()))
        })
}

pub(super) fn system_mcp_path(target: &Target) -> Result<PathBuf, AppError> {
    Ok(system_mcp_paths(target)?[0].clone())
}

pub(super) fn system_mcp_paths(target: &Target) -> Result<Vec<PathBuf>, AppError> {
    let directory = system_config_directory(target)?;
    Ok(match target.name {
        "claude" => vec![directory.join("managed-mcp.json")],
        "gemini" => vec![
            directory.join("settings.json"),
            directory.join("system-defaults.json"),
        ],
        "opencode" => ["config.json", "opencode.json", "opencode.jsonc"]
            .map(|name| directory.join(name))
            .to_vec(),
        "codex" => vec![directory.join("config.toml")],
        _ => vec![directory.join("mcp.json")],
    })
}

pub(super) fn system_config_directory(target: &Target) -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    {
        // Every agent keeps its machine-wide configuration under %ProgramData%,
        // the writable counterpart of the read-only install directory.
        let base = std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .ok_or_else(|| {
                AppError::external(
                    "AI_SYSTEM_CONFIG_UNRESOLVED",
                    format!("unable to resolve %ProgramData% for {}", target.name),
                )
            })?;
        Ok(base.join(match target.name {
            "claude" => "ClaudeCode",
            "gemini" => "gemini-cli",
            "opencode" => "opencode",
            _ => target.name,
        }))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(
            PathBuf::from("/Library/Application Support").join(match target.name {
                "claude" => "ClaudeCode",
                "gemini" => "GeminiCli",
                _ => target.name,
            }),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Ok(PathBuf::from("/etc").join(match target.name {
            "claude" => "claude-code",
            "gemini" => "gemini-cli",
            _ => target.name,
        }))
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
