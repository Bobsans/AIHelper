//! Reading back what is installed, per target and per scope, without changing
//! any of it.

use super::*;

pub(crate) fn status(
    request: StatusRequest,
    cwd: Option<&std::path::Path>,
) -> Result<StatusReport, AppError> {
    let selected: Vec<&Target> = match request.target {
        Some(name) => vec![targets::find(&name)?],
        None => targets::TARGETS.iter().collect(),
    };
    let cwd = project_root(cwd)?;
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

pub(crate) fn status_target(
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

pub(crate) fn status_scopes(target: &Target, in_project: bool) -> Vec<Scope> {
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
