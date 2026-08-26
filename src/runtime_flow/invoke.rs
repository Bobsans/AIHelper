//! Running the command the parse produced: resolving it against the catalog,
//! supplying its credentials, and turning a failure into a diagnostic.

use super::*;

pub(super) fn command_log_name(command: &RuntimeCommand, manager: &PluginManager) -> String {
    match command {
        RuntimeCommand::McpServe { .. } => "mcp.serve".to_owned(),
        RuntimeCommand::PluginsList { .. } => "plugins.list".to_owned(),
        RuntimeCommand::PluginsEnable { .. } => "plugins.enable".to_owned(),
        RuntimeCommand::PluginsDisable { .. } => "plugins.disable".to_owned(),
        RuntimeCommand::PluginsReset { .. } => "plugins.reset".to_owned(),
        RuntimeCommand::AiInfo { .. } => "ai.info".to_owned(),
        RuntimeCommand::Ai { request, .. } => match request {
            crate::ai::install::AiCommand::Install(_) => "ai.install",
            crate::ai::install::AiCommand::Uninstall(_) => "ai.uninstall",
            crate::ai::install::AiCommand::Status(_) => "ai.status",
        }
        .to_owned(),
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

pub(super) fn resolve_invocation_command(
    manager: &PluginManager,
    domain: &str,
    argv: &[String],
) -> String {
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

pub(super) fn plugin_source_name(source: PluginSource) -> &'static str {
    match source {
        PluginSource::Builtin => "builtin",
        PluginSource::Dynamic => "dynamic",
    }
}

pub(super) fn execution(
    command: RuntimeCommand,
    config: ConfigContext,
    manager: PluginManager,
    vault: Arc<crate::secrets::VaultStore>,
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
            manager,
            vault,
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
        RuntimeCommand::Ai { request, options } => {
            ai::install::execute(&manager, request, options).map(|_| None)
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
            let credential_args = extract_credential_args(argv)?;
            if !credential_args.credentials.is_empty() {
                require_secret_slots(&domain, command_catalog.as_ref())?;
            }
            let observation = manager
                .invoke_credentialed(
                    &domain,
                    credential_args.argv,
                    options.to_wire(),
                    &credential_args.credentials,
                )
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

pub(super) struct CredentialArgs {
    pub(super) argv: Vec<String>,
    pub(super) credentials: BTreeMap<String, String>,
}

/// Lifts `--credential SLOT=ID` out of the domain argv. Only options before the
/// `--` separator are recognized, so a literal value can always be passed with
/// the attached form of its own option, such as `--body=--credential=a=b`.
pub(super) fn extract_credential_args(argv: Vec<String>) -> Result<CredentialArgs, AppError> {
    let mut clean = Vec::with_capacity(argv.len());
    let mut credentials = BTreeMap::new();
    let mut index = 0;
    let mut positional_only = false;
    while index < argv.len() {
        if positional_only {
            clean.push(argv[index].clone());
            index += 1;
            continue;
        }
        if argv[index] == "--" {
            positional_only = true;
            clean.push(argv[index].clone());
            index += 1;
            continue;
        }
        let (mapping, consumed) = if argv[index] == "--credential" {
            (argv.get(index + 1).map(String::as_str), 2)
        } else if let Some(mapping) = argv[index].strip_prefix("--credential=") {
            (Some(mapping), 1)
        } else {
            clean.push(argv[index].clone());
            index += 1;
            continue;
        };
        let mapping = mapping
            .ok_or_else(|| AppError::invalid_argument("--credential requires a SLOT=ID value"))?;
        let Some((slot, id)) = mapping.split_once('=') else {
            return Err(AppError::invalid_argument("--credential must use SLOT=ID"));
        };
        if slot.trim().is_empty() || id.trim().is_empty() || id.contains('=') {
            return Err(AppError::invalid_argument(
                "--credential must use non-empty SLOT=ID",
            ));
        }
        match credentials.entry(slot.to_owned()) {
            Entry::Vacant(entry) => {
                entry.insert(id.to_owned());
            }
            Entry::Occupied(_) => {
                return Err(AppError::invalid_argument(format!(
                    "duplicate credential slot '{slot}'"
                )));
            }
        }
        index += consumed;
    }
    Ok(CredentialArgs {
        argv: clean,
        credentials,
    })
}

/// `--credential` is accepted for any domain whose catalog declares a secret
/// slot, so a new plugin needs no host-side allowlist entry.
pub(super) fn require_secret_slots(
    domain: &str,
    catalog: Option<&CommandCatalog>,
) -> Result<(), AppError> {
    let declares_slot = catalog.is_some_and(|catalog| {
        catalog
            .commands
            .iter()
            .any(|command| !command.secret_slots.is_empty())
    });
    if declares_slot {
        return Ok(());
    }
    Err(AppError::invalid_argument(format!(
        "domain '{domain}' does not accept --credential"
    )))
}

pub(super) fn decorate_invocation_error(
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
