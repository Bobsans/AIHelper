use std::path::PathBuf;

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
    prompt, registrar,
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

#[derive(Debug, Serialize)]
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
            let report = status(request)?;
            emit_status(&report, options)
        }
    }
}

fn project_root() -> Result<PathBuf, AppError> {
    std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source))
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
        if registrar::probe(target, scope, name, root)?.is_some() {
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
                if !dry_run {
                    if let Err(error) = registrar::run(&invocation) {
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
                }
                commands.push(invocation);
            }
            Registrar::File => {
                let config = target.json_config()?;
                let file = config.path(mcp_scope, root)?;
                if !dry_run {
                    let document =
                        json_config::merge(json_config::read(&file)?, &config, SERVER_NAME, spec);
                    json_config::write(&file, &document)?;
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
    let mut commands = Vec::new();
    let config_path = config_path_for(target, mcp_scope, &root)?;
    let mut mcp_action = if existing.is_some() {
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
    let root = project_root()?;
    let mut reports = Vec::new();
    for target in selected {
        let scope = target.default_scope;
        let mcp_scope = target.mcp_scope(scope);
        // A file-backed target has no CLI to look for, so it is always usable.
        let available = match target.cli_program() {
            Some(program) => cli_available(program),
            None => true,
        };
        let existing = if available {
            registrar::probe(target, mcp_scope, SERVER_NAME, &root)?
        } else {
            None
        };
        let legacy = if available {
            legacy_registration(target, mcp_scope, &root)?
        } else {
            None
        };
        let config_path = config_path_for(target, mcp_scope, &root)?;
        let path = target.rules_path(scope, &root)?;
        let hint = path.display().to_string();
        let rules_action = match rules::read(&path)? {
            Some(contents) if contents.contains(rules::BEGIN_MARKER) => Action::Installed,
            _ => Action::NotPresent,
        };
        reports.push(TargetStatus {
            target: target.name,
            scope,
            cli: target.cli_program(),
            cli_available: available,
            legacy_server: legacy,
            mcp: McpReport {
                action: if existing.is_some() {
                    Action::Installed
                } else {
                    Action::NotPresent
                },
                registrar: registrar_label(target),
                scope: mcp_scope,
                transport: existing.as_ref().map(ServerSpec::transport),
                url: existing
                    .as_ref()
                    .and_then(ServerSpec::url)
                    .map(str::to_owned),
                path: config_path,
                commands: Vec::new(),
            },
            rules: RulesReport {
                action: rules_action,
                path: hint,
            },
        });
    }
    Ok(StatusReport {
        command: "ai.status",
        schema_version: SCHEMA_VERSION,
        targets: reports,
    })
}

fn cli_available(program: &str) -> bool {
    registrar::run(&registrar::Invocation {
        program: program.to_owned(),
        args: vec!["--version".to_owned()],
    })
    .is_ok()
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
                let transport = entry
                    .mcp
                    .transport
                    .map(|transport| format!(", {}", transport.as_str()))
                    .unwrap_or_default();
                println!(
                    "  mcp   {} (scope {}{transport})",
                    formatter.paint(
                        action_style(entry.mcp.action),
                        action_label(entry.mcp.action)
                    ),
                    entry.mcp.scope.as_str(),
                );
                println!(
                    "  rules {} ({})",
                    formatter.paint(
                        action_style(entry.rules.action),
                        action_label(entry.rules.action)
                    ),
                    formatter.paint(TextStyle::Key, &entry.rules.path),
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_HTTP_URL, http_spec};
    use crate::ai::targets::ServerSpec;

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
}
