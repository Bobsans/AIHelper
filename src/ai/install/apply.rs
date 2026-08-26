//! The mutations: registering the MCP server, writing the rules block, and
//! removing either again.

use super::*;

pub(crate) fn install(
    manager: &PluginManager,
    mut request: InstallRequest,
    cwd: Option<&std::path::Path>,
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
    let root = project_root(cwd)?;
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

/// A registration under a superseded server name, if the agent still has one.
pub(crate) fn legacy_registration(
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

pub(crate) fn uninstall(
    request: UninstallRequest,
    cwd: Option<&std::path::Path>,
) -> Result<TargetReport, AppError> {
    let target = targets::find(&request.target)?;
    let scope = resolve_scope(target, request.scope)?;
    let root = project_root(cwd)?;
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
