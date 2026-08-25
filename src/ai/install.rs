use std::{
    borrow::Cow,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

use ah_runtime::PluginManager;
use serde::Serialize;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::{OutputMode, TextFormatter, TextStyle, emit_warning},
};

use super::{
    json_config,
    managed::{self, ManagedAction},
    opencode_config, prompt, registrar,
    rules::{self, BlockAction as Action},
    targets::{
        self, LEGACY_SERVER_NAMES, Registrar, SERVER_NAME, Scope, ServerSpec, Target, Transport,
    },
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
struct McpReport {
    action: Action,
    registrar: &'static str,
    scope: Scope,
    transport: Option<Transport>,
    url: Option<String>,
    path: Option<String>,
    commands: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RulesReport {
    action: Action,
    path: String,
}

#[derive(Debug, Serialize)]
struct ManagedReport {
    action: ManagedAction,
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct TargetReport {
    command: &'static str,
    schema_version: u32,
    target: &'static str,
    scope: Scope,
    changed: bool,
    dry_run: bool,
    mcp: McpReport,
    rules: RulesReport,
    managed_service: Option<ManagedReport>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct StatusReport {
    command: &'static str,
    schema_version: u32,
    targets: Vec<TargetStatus>,
}

#[derive(Debug, Serialize)]
struct TargetStatus {
    target: &'static str,
    scope: Scope,
    cli: Option<&'static str>,
    cli_available: bool,
    /// A registration left by an older AIHelper under its previous server name.
    legacy_server: Option<&'static str>,
    mcp: McpReport,
    rules: RulesReport,
    scopes: Vec<ScopeStatus>,
}

#[derive(Debug, Serialize)]
struct ScopeStatus {
    scope: Scope,
    mcp: Option<ScopedMcpReport>,
    rules: Option<ScopedRulesReport>,
}

#[derive(Debug, Clone, Serialize)]
struct ScopedMcpReport {
    action: StatusAction,
    registrar: &'static str,
    scope: Scope,
    transport: Option<Transport>,
    url: Option<String>,
    path: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ScopedRulesReport {
    action: StatusAction,
    path: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum StatusAction {
    Installed,
    NotPresent,
    Unknown,
}

const SCHEMA_VERSION: u32 = 1;

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

fn project_root() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))
}

fn status_project_root(cwd: &std::path::Path) -> Option<PathBuf> {
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

fn resolve_scope(target: &Target, requested: Option<Scope>) -> Result<Scope, AppError> {
    let scope = requested.unwrap_or(target.default_scope);
    target.require_scope(scope)?;
    Ok(scope)
}

fn stdio_spec() -> Result<ServerSpec, AppError> {
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
fn http_spec(url: Option<&str>) -> Result<ServerSpec, AppError> {
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

fn rules_block(manager: &PluginManager) -> String {
    let mut domains = manager
        .collect_plugin_manuals()
        .into_iter()
        .map(|manual| manual.domain)
        .collect::<Vec<_>>();
    domains.sort();
    domains.dedup();
    rules::render_block(&domains)
}

fn install(manager: &PluginManager, mut request: InstallRequest) -> Result<TargetReport, AppError> {
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
fn plan_managed() -> Result<managed::Snapshot, AppError> {
    let snapshot = managed::detect()?;
    managed::require_usable(&snapshot)?;
    Ok(snapshot)
}

fn plan_summary(
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

fn cancelled_report(target: &Target, scope: Scope, rules_path: String) -> TargetReport {
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
fn probe_readiness(spec: &ServerSpec) -> Option<String> {
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

fn registrar_label(target: &Target) -> &'static str {
    match target.registrar {
        Registrar::Cli { .. } => "cli",
        Registrar::File => "file",
    }
}

fn skipped_mcp(target: &Target, scope: Scope) -> McpReport {
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

fn config_path_for(
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
fn legacy_registration(
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
fn apply_remove(
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

fn install_mcp(
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

fn install_rules(
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

fn uninstall(request: UninstallRequest) -> Result<TargetReport, AppError> {
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

fn status(request: StatusRequest) -> Result<StatusReport, AppError> {
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

fn status_target(
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

fn status_scopes(target: &Target, in_project: bool) -> Vec<Scope> {
    [Scope::System, Scope::User, Scope::Project, Scope::Local]
        .into_iter()
        .filter(|scope| target.supports_status_mcp(*scope) || target.supports_status_rules(*scope))
        .filter(|scope| matches!(scope, Scope::System | Scope::User) || in_project)
        .collect()
}

fn scoped_rules_status(
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

fn scoped_mcp_status(
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

fn system_mcp_status(target: &Target) -> Result<(Option<ServerSpec>, PathBuf), AppError> {
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

fn copilot_user_mcp_path() -> Result<PathBuf, AppError> {
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

fn codex_config_path(scope: Scope, root: &std::path::Path) -> Result<PathBuf, AppError> {
    match scope {
        Scope::System => system_mcp_path(targets::find("codex")?),
        Scope::User => Ok(targets::home_dir()?.join(".codex").join("config.toml")),
        Scope::Project | Scope::Local => Ok(root.join(".codex").join("config.toml")),
    }
}

fn codex_mcp_entry(path: &std::path::Path, name: &str) -> Result<Option<ServerSpec>, AppError> {
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

fn toml_string(line: &str, key: &str) -> Option<String> {
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

fn system_mcp_path(target: &Target) -> Result<PathBuf, AppError> {
    Ok(system_mcp_paths(target)?[0].clone())
}

fn system_mcp_paths(target: &Target) -> Result<Vec<PathBuf>, AppError> {
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

fn system_config_directory(target: &Target) -> Result<PathBuf, AppError> {
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

fn parallel_map<T: Sync, R: Send>(values: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
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

struct LiveTarget {
    target: &'static Target,
    cli: Option<(Option<&'static str>, bool)>,
    scopes: Vec<LiveScope>,
    legacy_server: Option<&'static str>,
    error: bool,
}

struct LiveScope {
    scope: Scope,
    mcp_supported: bool,
    mcp: Option<ScopedMcpReport>,
    rules_supported: bool,
    rules: Option<ScopedRulesReport>,
}

impl LiveTarget {
    fn new(target: &'static Target, in_project: bool) -> Self {
        Self {
            target,
            cli: None,
            scopes: status_scopes(target, in_project)
                .into_iter()
                .map(|scope| LiveScope {
                    scope,
                    mcp_supported: target.supports_status_mcp(scope),
                    mcp: None,
                    rules_supported: target.supports_status_rules(scope),
                    rules: None,
                })
                .collect(),
            legacy_server: None,
            error: false,
        }
    }

    fn apply(&mut self, progress: StatusProgress) {
        match progress {
            StatusProgress::Cli { cli, available } => self.cli = Some((cli, available)),
            StatusProgress::Mcp { scope, report } => {
                self.scopes
                    .iter_mut()
                    .find(|status| status.scope == scope)
                    .expect("status scope should be preallocated")
                    .mcp = Some(report)
            }
            StatusProgress::Rules { scope, report } => {
                self.scopes
                    .iter_mut()
                    .find(|status| status.scope == scope)
                    .expect("status scope should be preallocated")
                    .rules = Some(report)
            }
        }
    }

    fn complete(&mut self, result: &Result<TargetStatus, AppError>) {
        match result {
            Ok(status) => self.legacy_server = status.legacy_server,
            Err(_) => self.error = true,
        }
    }
}

enum StatusProgress {
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

enum LiveEvent {
    Progress(usize, StatusProgress),
    Complete(usize, Result<TargetStatus, AppError>),
}

struct CursorGuard;

impl Drop for CursorGuard {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = write!(output, "\u{1b}[?25h");
        let _ = output.flush();
    }
}

/// One scope of one target: `mcp` and `rules` side by side under a heading.
struct LiveRow {
    target: usize,
    scope: &'static str,
    mcp: String,
    rules: String,
}

const UNSUPPORTED: &str = "not supported";

/// Widest cell a finished `mcp` check can produce, reserved before the first
/// frame so the `rules` column never shifts as spinners become results.
fn mcp_width() -> usize {
    let transport = [Transport::Stdio, Transport::Http]
        .into_iter()
        .map(|transport| transport.as_str().len())
        .max()
        .unwrap_or(0);
    [
        StatusAction::Installed,
        StatusAction::NotPresent,
        StatusAction::Unknown,
    ]
    .into_iter()
    .map(|action| {
        let label = status_action_label(action).len();
        // Only a registration carries a transport in parentheses.
        match action {
            StatusAction::Installed => label + " (".len() + transport + ")".len(),
            _ => label,
        }
    })
    .chain([UNSUPPORTED.len()])
    .max()
    .unwrap_or(UNSUPPORTED.len())
}

fn render_live_lines(
    targets: &[LiveTarget],
    spinner: &str,
    formatter: TextFormatter,
) -> Vec<String> {
    let error = formatter.paint(TextStyle::Error, "error");
    let unsupported = formatter.paint(TextStyle::Muted, UNSUPPORTED);
    let headings = targets.iter().map(|target| {
        let cli = match target.cli {
            Some((Some(cli), true)) => {
                formatter.paint(TextStyle::Success, format!("`{cli}` available"))
            }
            Some((Some(cli), false)) => {
                formatter.paint(TextStyle::Warning, format!("`{cli}` missing"))
            }
            Some((None, _)) => formatter.paint(TextStyle::Muted, "config file"),
            None if target.error => error.clone(),
            None => spinner.to_owned(),
        };
        // A legacy registration belongs to the target rather than to one of its
        // scopes, and would widen the mcp column it used to sit in.
        let legacy = target
            .legacy_server
            .map(|name| formatter.paint(TextStyle::Warning, format!(", legacy `{name}` present")))
            .unwrap_or_default();
        format!(
            "{} ({cli}{legacy})",
            formatter.paint(TextStyle::Heading, target.target.name),
        )
    });
    let error = error.as_str();
    let unsupported = unsupported.as_str();
    let rows: Vec<LiveRow> = targets
        .iter()
        .enumerate()
        .flat_map(|(index, target)| {
            let pending = move || {
                if target.error {
                    error.to_owned()
                } else {
                    spinner.to_owned()
                }
            };
            target.scopes.iter().map(move |scope| {
                let mcp = if !scope.mcp_supported {
                    unsupported.to_owned()
                } else if let Some(mcp) = &scope.mcp {
                    let details: Vec<String> = mcp
                        .transport
                        .map(|transport| transport.as_str().to_owned())
                        .into_iter()
                        .chain(mcp.detail.clone())
                        .collect();
                    state_cell(mcp.action, &details, formatter)
                } else {
                    pending()
                };
                let rules = if !scope.rules_supported {
                    unsupported.to_owned()
                } else if let Some(rules) = &scope.rules {
                    state_cell(rules.action, rules.detail.as_slice(), formatter)
                } else {
                    pending()
                };
                LiveRow {
                    target: index,
                    scope: scope.scope.as_str(),
                    mcp,
                    rules,
                }
            })
        })
        .collect();
    let scope_width = rows.iter().map(|row| row.scope.len()).max().unwrap_or(0);
    let mcp_width = mcp_width();
    let separator = formatter.paint(TextStyle::Muted, "::");
    let mut lines = Vec::with_capacity(targets.len() + rows.len());
    for (index, heading) in headings.enumerate() {
        lines.push(heading);
        for row in rows.iter().filter(|row| row.target == index) {
            lines.push(format!(
                "  {} {separator} mcp {} rules {}",
                pad(&formatter.paint(TextStyle::Key, row.scope), scope_width),
                pad(&row.mcp, mcp_width),
                row.rules,
            ));
        }
    }
    lines
}

fn state_cell(action: StatusAction, details: &[String], formatter: TextFormatter) -> String {
    let state = formatter.paint(status_action_style(action), status_action_label(action));
    if details.is_empty() {
        return state;
    }
    format!("{state} ({})", details.join(", "))
}

/// Pads to a visible width, which `{:<width$}` cannot do once a cell carries
/// colour escapes.
fn pad(cell: &str, width: usize) -> String {
    let padding = width.saturating_sub(console::measure_text_width(cell));
    format!("{cell}{:padding$}", "")
}

/// A live block redrawn in place, kept inside the terminal it lands in.
struct LiveScreen {
    /// Widest column a line may occupy before it wraps and desynchronises the
    /// cursor arithmetic below.
    width: Option<usize>,
    /// Tallest block that may be animated: once the block scrolls, its first row
    /// is out of reach, `\u{1b}[<n>A` clamps at the top of the screen and every
    /// frame is appended instead of replacing the previous one.
    height: Option<usize>,
    drawn: usize,
}

impl LiveScreen {
    fn new() -> Self {
        let size = console::Term::stdout().size_checked();
        Self {
            // Terminals differ on whether a line filling the last column wraps
            // immediately, so keep one column and one row spare.
            width: size.map(|(_, width)| usize::from(width).saturating_sub(1)),
            height: size.map(|(height, _)| usize::from(height).saturating_sub(1)),
            drawn: 0,
        }
    }

    /// One animation frame, clipped to the visible viewport.
    fn draw(&mut self, output: &mut impl Write, lines: &[String]) -> io::Result<()> {
        let visible = clip_height(lines, self.height);
        self.rewind(output)?;
        write_live_lines(output, &visible, self.width)?;
        self.drawn = visible.len();
        output.flush()
    }

    /// The finished report, in full: the clipped frames are erased first so the
    /// rows the viewport could not hold are not left behind as a stale copy.
    fn finish(&mut self, output: &mut impl Write, lines: &[String]) -> io::Result<()> {
        self.rewind(output)?;
        if self.drawn > 0 {
            write!(output, "\r\u{1b}[J")?;
        }
        write_live_lines(output, lines, self.width)?;
        self.drawn = lines.len();
        output.flush()
    }

    fn rewind(&self, output: &mut impl Write) -> io::Result<()> {
        if self.drawn > 0 {
            write!(output, "\u{1b}[{}A", self.drawn)?;
        }
        Ok(())
    }
}

/// Keeps the block inside `height` rows, replacing the rows that do not fit with
/// a count of what the finished report will add.
fn clip_height(lines: &[String], height: Option<usize>) -> Vec<String> {
    let Some(height) = height.filter(|height| lines.len() > *height) else {
        return lines.to_vec();
    };
    let kept = height.saturating_sub(1);
    let mut visible = lines[..kept].to_vec();
    if height > 0 {
        visible.push(format!("… {} more", lines.len() - kept));
    }
    visible
}

fn write_live_lines(
    output: &mut impl Write,
    lines: &[String],
    width: Option<usize>,
) -> io::Result<()> {
    for line in lines {
        let line = match width {
            Some(width) => console::truncate_str(line, width, ""),
            None => Cow::Borrowed(line.as_str()),
        };
        // Overwrite in place and clear the tail: clearing first would blank the
        // row for a frame and read as flicker.
        write!(output, "\r{line}\u{1b}[K\n")?;
    }
    Ok(())
}

fn emit_live_status(request: StatusRequest) -> Result<(), AppError> {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    const FRAME_INTERVAL: Duration = Duration::from_millis(80);

    let selected: Vec<&Target> = match request.target {
        Some(name) => vec![targets::find(&name)?],
        None => targets::TARGETS.iter().collect(),
    };
    let cwd = project_root()?;
    let project = status_project_root(&cwd);
    let in_project = project.is_some();
    let root = project.as_deref().unwrap_or(&cwd);
    let mut targets = selected
        .iter()
        .map(|target| LiveTarget::new(target, in_project))
        .collect::<Vec<_>>();
    let mut results = std::iter::repeat_with(|| None)
        .take(selected.len())
        .collect::<Vec<Option<Result<TargetStatus, AppError>>>>();
    let formatter = TextFormatter::stdout();
    let mut output = io::stdout();
    let started = Instant::now();
    let frame = |elapsed: Duration| {
        FRAMES[(elapsed.as_millis() / FRAME_INTERVAL.as_millis()) as usize % FRAMES.len()]
    };
    let mut screen = LiveScreen::new();
    write!(output, "\u{1b}[?25l").map_err(status_render_error)?;
    let _cursor = CursorGuard;
    screen
        .draw(
            &mut output,
            &render_live_lines(&targets, FRAMES[0], formatter),
        )
        .map_err(status_render_error)?;

    std::thread::scope(|scope| -> Result<(), AppError> {
        let (sender, receiver) = mpsc::channel();
        for (index, target) in selected.iter().copied().enumerate() {
            let sender = sender.clone();
            let root = &root;
            scope.spawn(move || {
                let result = status_target(target, root, in_project, |progress| {
                    let _ = sender.send(LiveEvent::Progress(index, progress));
                });
                let _ = sender.send(LiveEvent::Complete(index, result));
            });
        }
        drop(sender);

        let mut completed = 0;
        while completed < selected.len() {
            match receiver.recv_timeout(FRAME_INTERVAL) {
                Ok(LiveEvent::Progress(index, progress)) => targets[index].apply(progress),
                Ok(LiveEvent::Complete(index, result)) => {
                    targets[index].complete(&result);
                    if results[index].is_none() {
                        completed += 1;
                    }
                    results[index] = Some(result);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AppError::external(
                        "AI_STATUS_WORKER_FAILED",
                        "an AI status worker stopped without returning a result",
                    ));
                }
            }
            screen
                .draw(
                    &mut output,
                    &render_live_lines(&targets, frame(started.elapsed()), formatter),
                )
                .map_err(status_render_error)?;
        }
        Ok(())
    })?;

    screen
        .finish(
            &mut output,
            &render_live_lines(&targets, FRAMES[0], formatter),
        )
        .map_err(status_render_error)?;

    results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|| {
                Err(AppError::external(
                    "AI_STATUS_WORKER_FAILED",
                    "an AI status worker stopped without returning a result",
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(())
}

fn status_render_error(source: io::Error) -> AppError {
    AppError::external(
        "AI_STATUS_RENDER_FAILED",
        format!("unable to render AI status: {source}"),
    )
}

fn cli_available(program: &str) -> bool {
    registrar::available(program)
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::Installed => "installed",
        Action::Updated => "updated",
        Action::Unchanged => "unchanged",
        Action::Removed => "removed",
        Action::NotPresent => "not present",
        Action::Skipped => "skipped",
    }
}

fn action_style(action: Action) -> TextStyle {
    match action {
        Action::Installed | Action::Updated | Action::Removed => TextStyle::Success,
        Action::Unchanged | Action::NotPresent | Action::Skipped => TextStyle::Muted,
    }
}

fn status_action_label(action: StatusAction) -> &'static str {
    match action {
        StatusAction::Installed => "installed",
        StatusAction::NotPresent => "not present",
        StatusAction::Unknown => "unknown",
    }
}

fn status_action_style(action: StatusAction) -> TextStyle {
    match action {
        StatusAction::Installed => TextStyle::Success,
        StatusAction::NotPresent | StatusAction::Unknown => TextStyle::Muted,
    }
}

fn emit(report: &TargetReport, options: GlobalOptions) -> Result<(), AppError> {
    for warning in &report.warnings {
        emit_warning(warning);
    }
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(report)?),
        OutputMode::Text => {
            let formatter = TextFormatter::stdout();
            let prefix = if report.dry_run { "would be " } else { "" };
            let mut detail = format!("scope {}", report.mcp.scope.as_str());
            if let Some(transport) = report.mcp.transport {
                detail = format!("{}, {detail}", transport.as_str());
            }
            println!(
                "{} mcp {}{} ({detail})",
                formatter.paint(TextStyle::Heading, report.target),
                prefix,
                formatter.paint(
                    action_style(report.mcp.action),
                    action_label(report.mcp.action)
                ),
            );
            println!(
                "{} rules {}{} ({})",
                formatter.paint(TextStyle::Heading, report.target),
                prefix,
                formatter.paint(
                    action_style(report.rules.action),
                    action_label(report.rules.action)
                ),
                formatter.paint(TextStyle::Key, &report.rules.path),
            );
            if let Some(managed) = &report.managed_service {
                println!(
                    "{} managed service {}{} ({})",
                    formatter.paint(TextStyle::Heading, report.target),
                    prefix,
                    formatter.paint(TextStyle::Success, managed.action.as_str()),
                    formatter.paint(TextStyle::Key, &managed.endpoint),
                );
            }
            for invocation in &report.mcp.commands {
                println!(
                    "  {} {}",
                    formatter.paint(
                        TextStyle::Muted,
                        if report.dry_run { "would run" } else { "ran" }
                    ),
                    formatter.paint(TextStyle::Muted, invocation),
                );
            }
        }
    }
    Ok(())
}

fn emit_status(report: &StatusReport, options: GlobalOptions) -> Result<(), AppError> {
    if options.quiet {
        return Ok(());
    }
    match options.output {
        OutputMode::Json => println!("{}", serde_json::to_string_pretty(report)?),
        OutputMode::Text => {
            let formatter = TextFormatter::stdout();
            for entry in &report.targets {
                let cli_state = match entry.cli {
                    Some(cli) if entry.cli_available => {
                        formatter.paint(TextStyle::Success, format!("`{cli}` available"))
                    }
                    Some(cli) => formatter.paint(TextStyle::Warning, format!("`{cli}` missing")),
                    None => formatter.paint(TextStyle::Muted, "config file"),
                };
                println!(
                    "{} ({cli_state})",
                    formatter.paint(TextStyle::Heading, entry.target)
                );
                if let Some(legacy) = entry.legacy_server {
                    println!(
                        "  {}",
                        formatter.paint(
                            TextStyle::Warning,
                            format!(
                                "legacy `{legacy}` registration present;                                  `ah ai install` or `ah ai uninstall` removes it"
                            )
                        )
                    );
                }
                for scope in &entry.scopes {
                    println!(
                        "  {}",
                        formatter.paint(TextStyle::Key, scope.scope.as_str())
                    );
                    if let Some(mcp) = &scope.mcp {
                        let detail = mcp
                            .transport
                            .map(|transport| transport.as_str())
                            .or(mcp.detail.as_deref())
                            .map(|detail| format!(" ({detail})"))
                            .unwrap_or_default();
                        println!(
                            "    mcp   {}{detail}",
                            formatter.paint(
                                status_action_style(mcp.action),
                                status_action_label(mcp.action)
                            ),
                        );
                    } else {
                        println!(
                            "    mcp   {}",
                            formatter.paint(TextStyle::Muted, "not supported")
                        );
                    }
                    if let Some(rules) = &scope.rules {
                        let detail = rules
                            .path
                            .as_deref()
                            .or(rules.detail.as_deref())
                            .map(|detail| format!(" ({})", formatter.paint(TextStyle::Key, detail)))
                            .unwrap_or_default();
                        println!(
                            "    rules {}{detail}",
                            formatter.paint(
                                status_action_style(rules.action),
                                status_action_label(rules.action)
                            ),
                        );
                    } else {
                        println!(
                            "    rules {}",
                            formatter.paint(TextStyle::Muted, "not supported")
                        );
                    }
                }
            }
        }
    }
    Ok(())
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

    use std::io;

    use super::{
        DEFAULT_HTTP_URL, LiveScreen, LiveTarget, ScopedMcpReport, StatusAction, StatusProgress,
        http_spec, parallel_map, render_live_lines, status_target,
    };
    use crate::ai::targets::{Scope, ServerSpec, Transport};
    use crate::output::TextFormatter;

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
    fn pending_status_renders_each_supported_scope() {
        let targets = [LiveTarget::new(
            crate::ai::targets::find("cursor").unwrap(),
            true,
        )];

        assert_eq!(
            render_live_lines(&targets, "*", TextFormatter::with_color(false)),
            vec![
                "cursor (*)",
                "  user    :: mcp *                 rules *",
                "  project :: mcp *                 rules *",
            ]
        );
    }

    #[test]
    fn live_status_replaces_ready_slots_without_waiting_for_mcp() {
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        target.apply(StatusProgress::Cli {
            cli: None,
            available: true,
        });
        target.apply(StatusProgress::Rules {
            scope: Scope::User,
            report: super::ScopedRulesReport {
                action: super::StatusAction::Unknown,
                path: None,
                detail: Some("Cursor settings".to_owned()),
            },
        });

        assert_eq!(
            render_live_lines(&[target], "*", TextFormatter::with_color(false)),
            vec![
                "cursor (config file)",
                "  user    :: mcp *                 rules unknown (Cursor settings)",
                "  project :: mcp *                 rules *",
            ]
        );
    }

    #[test]
    fn live_status_replaces_the_mcp_slot_when_its_probe_finishes() {
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        target.apply(StatusProgress::Mcp {
            scope: Scope::Project,
            report: ScopedMcpReport {
                action: StatusAction::Installed,
                registrar: "file",
                scope: Scope::Project,
                transport: Some(Transport::Http),
                url: Some("http://127.0.0.1:8787/mcp".to_owned()),
                path: Some("opencode.json".to_owned()),
                detail: None,
            },
        });

        assert_eq!(
            render_live_lines(&[target], "*", TextFormatter::with_color(false))[2],
            "  project :: mcp installed (http)  rules *"
        );
    }

    #[test]
    fn the_rules_column_does_not_move_when_a_spinner_becomes_a_result() {
        let formatter = TextFormatter::with_color(false);
        let mut target = LiveTarget::new(crate::ai::targets::find("cursor").unwrap(), true);
        let pending = render_live_lines(std::slice::from_ref(&target), "*", formatter);
        target.apply(StatusProgress::Mcp {
            scope: Scope::User,
            report: ScopedMcpReport {
                action: StatusAction::Installed,
                registrar: "file",
                scope: Scope::User,
                transport: Some(Transport::Stdio),
                url: None,
                path: None,
                detail: None,
            },
        });

        let column = |lines: &[String]| {
            lines[1..]
                .iter()
                .map(|line| line.find("rules ").expect("every row has a rules cell"))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            column(&pending),
            column(&render_live_lines(&[target], "*", formatter))
        );
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

    fn screen(width: Option<usize>, height: Option<usize>) -> LiveScreen {
        LiveScreen {
            width,
            height,
            drawn: 0,
        }
    }

    #[test]
    fn the_first_frame_is_written_where_the_cursor_already_is() {
        let mut output = Vec::new();

        screen(None, None)
            .draw(&mut output, &["one".to_owned(), "two".to_owned()])
            .unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\rone\u{1b}[K\n\rtwo\u{1b}[K\n"
        );
    }

    #[test]
    fn every_later_frame_rewinds_over_the_rows_it_drew() {
        let mut output = Vec::new();
        let mut screen = screen(None, None);
        let lines = ["one".to_owned(), "two".to_owned()];

        screen.draw(&mut io::sink(), &lines).unwrap();
        screen.draw(&mut output, &lines).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}[2A\rone\u{1b}[K\n\rtwo\u{1b}[K\n"
        );
    }

    #[test]
    fn lines_are_clipped_to_the_terminal_width_ansi_aside() {
        let mut output = Vec::new();

        screen(Some(9), None)
            .draw(&mut output, &["\u{1b}[1msome very long line".to_owned()])
            .unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(
            rendered.contains("some very") && !rendered.contains("long"),
            "{rendered:?} should keep nine visible columns"
        );
    }

    #[test]
    fn a_block_taller_than_the_terminal_is_animated_within_the_viewport() {
        let lines: Vec<String> = (0..8).map(|row| format!("row {row}")).collect();

        assert_eq!(
            super::clip_height(&lines, Some(3)),
            vec!["row 0", "row 1", "… 6 more"]
        );
        assert_eq!(super::clip_height(&lines, Some(8)), lines);
        assert_eq!(super::clip_height(&lines, None), lines);
    }

    #[test]
    fn the_finished_report_erases_the_clipped_frames_before_printing_in_full() {
        let mut output = Vec::new();
        let mut screen = screen(None, Some(2));
        let lines = ["one".to_owned(), "two".to_owned(), "three".to_owned()];

        screen.draw(&mut io::sink(), &lines).unwrap();
        screen.finish(&mut output, &lines).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\u{1b}[2A\r\u{1b}[J\rone\u{1b}[K\n\rtwo\u{1b}[K\n\rthree\u{1b}[K\n"
        );
    }
}
