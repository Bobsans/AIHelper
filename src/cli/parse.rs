//! Turning one `ArgMatches` into a `RuntimeCommand`.

use super::*;

pub fn parse_runtime_command(
    mut raw_args: Vec<OsString>,
    plugins: &[PluginMetadata],
) -> Result<CliParseResult, AppError> {
    raw_args = redact_secret_command_argv(&raw_args);
    let run_check_argv = prepare_run_check_passthrough(&mut raw_args)?;
    let diagnostic_args = raw_args.clone();
    let mut command = build_cli_command(plugins);
    let matches = match command.try_get_matches_from_mut(raw_args) {
        Ok(matches) => matches,
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                error
                    .print()
                    .map_err(|io_error| AppError::invalid_argument(io_error.to_string()))?;
                return Ok(CliParseResult::ExitSuccess);
            }
            _ => {
                let error = AppError::invalid_argument(error.to_string());
                return Err(decorate_cli_parse_error(error, &command, &diagnostic_args));
            }
        },
    };

    let mut options = crate::entry::global_options(&matches)?;
    let runtime_command = match matches.subcommand() {
        Some((HOST_COMMAND_MCP, mcp_matches)) => {
            let Some((subcommand, mcp_submatches)) = mcp_matches.subcommand() else {
                return Err(AppError::invalid_argument("missing mcp subcommand"));
            };
            match subcommand {
                "serve" => {
                    if mcp_submatches.get_one::<usize>("max-queued").is_some() {
                        return Err(AppError::invalid_argument(
                            "--max-queued was removed because MCP execution is no longer queued; use --max-active",
                        ));
                    }
                    if options.output == OutputMode::Json {
                        return Err(AppError::invalid_argument(
                            "--json cannot be used with mcp serve because stdout is the MCP transport",
                        ));
                    }
                    let transport = match mcp_submatches
                        .get_one::<String>("transport")
                        .map(String::as_str)
                        .expect("mcp transport has a default")
                    {
                        "stdio" => McpTransport::Stdio,
                        "http" => McpTransport::Http,
                        value => {
                            return Err(AppError::invalid_argument(format!(
                                "unsupported MCP transport: {value}"
                            )));
                        }
                    };
                    let explicit_port = mcp_submatches.get_one::<u16>("port").copied();
                    if transport == McpTransport::Stdio && explicit_port.is_some() {
                        return Err(AppError::invalid_argument(
                            "--port can be used only with --transport http",
                        ));
                    }
                    let port = explicit_port.unwrap_or(8787);
                    if port == 0 {
                        return Err(AppError::invalid_argument("--port must be >= 1"));
                    }
                    let max_active = *mcp_submatches
                        .get_one::<usize>("max-active")
                        .expect("mcp max active count has a default");
                    if max_active == 0 {
                        return Err(AppError::invalid_argument("--max-active must be >= 1"));
                    }
                    let default_timeout_ms = *mcp_submatches
                        .get_one::<u64>("default-timeout-ms")
                        .expect("mcp timeout has a default");
                    if default_timeout_ms == 0 {
                        return Err(AppError::invalid_argument(
                            "--default-timeout-ms must be >= 1",
                        ));
                    }
                    RuntimeCommand::McpServe {
                        transport,
                        port,
                        max_active,
                        default_timeout_ms,
                        options,
                    }
                }
                _ => return Err(AppError::invalid_argument("unsupported mcp subcommand")),
            }
        }
        Some((HOST_COMMAND_AI, ai_matches)) => {
            let Some((subcommand, ai_submatches)) = ai_matches.subcommand() else {
                return Err(AppError::invalid_argument("missing ai subcommand"));
            };
            match subcommand {
                "info" => RuntimeCommand::AiInfo {
                    domain: ai_submatches.get_one::<String>("domain").cloned(),
                    options,
                },
                "install" => RuntimeCommand::Ai {
                    request: AiCommand::Install(InstallRequest {
                        target: required_target(ai_submatches)?,
                        scope: parse_scope(ai_submatches)?,
                        transport: parse_transport(ai_submatches)?,
                        managed: ai_submatches
                            .get_one::<String>("transport")
                            .is_some_and(|value| value == "managed"),
                        url: ai_submatches.get_one::<String>("url").cloned(),
                        with_mcp: !ai_submatches.get_flag("rules-only"),
                        with_rules: !ai_submatches.get_flag("mcp-only"),
                        dry_run: ai_submatches.get_flag("dry-run"),
                        interactive: crate::ai::prompt_wanted(has_decision_flags(
                            ai_submatches,
                            &options,
                        )),
                        assume_yes: ai_submatches.get_flag("yes"),
                    }),
                    options,
                },
                "uninstall" => RuntimeCommand::Ai {
                    request: AiCommand::Uninstall(UninstallRequest {
                        target: required_target(ai_submatches)?,
                        scope: parse_scope(ai_submatches)?,
                        dry_run: ai_submatches.get_flag("dry-run"),
                    }),
                    options,
                },
                "status" => RuntimeCommand::Ai {
                    request: AiCommand::Status(StatusRequest {
                        target: ai_submatches.get_one::<String>("target").cloned(),
                    }),
                    options,
                },
                _ => return Err(AppError::invalid_argument("unsupported ai subcommand")),
            }
        }
        Some((HOST_COMMAND_PLUGINS, plugins_matches)) => {
            let Some((subcommand, plugin_submatches)) = plugins_matches.subcommand() else {
                return Err(AppError::invalid_argument("missing plugins subcommand"));
            };
            match subcommand {
                "list" => {
                    let state_filter = plugin_submatches
                        .get_one::<String>("state")
                        .map(|value| parse_plugin_state_filter(value))
                        .transpose()?;
                    RuntimeCommand::PluginsList {
                        state_filter,
                        options,
                    }
                }
                "enable" => RuntimeCommand::PluginsEnable {
                    domain: plugin_submatches
                        .get_one::<String>("domain")
                        .cloned()
                        .ok_or_else(|| {
                            AppError::invalid_argument("missing plugins enable domain")
                        })?,
                    options,
                },
                "disable" => RuntimeCommand::PluginsDisable {
                    domain: plugin_submatches
                        .get_one::<String>("domain")
                        .cloned()
                        .ok_or_else(|| {
                            AppError::invalid_argument("missing plugins disable domain")
                        })?,
                    options,
                },
                "reset" => RuntimeCommand::PluginsReset {
                    domain: plugin_submatches.get_one::<String>("domain").cloned(),
                    all: plugin_submatches.get_flag("all"),
                    options,
                },
                _ => return Err(AppError::invalid_argument("unsupported plugins subcommand")),
            }
        }
        Some((HOST_COMMAND_SECRETS, secrets_matches)) => {
            let Some((subcommand, secret_matches)) = secrets_matches.subcommand() else {
                return Err(AppError::invalid_argument("missing secrets subcommand"));
            };
            let request = match subcommand {
                "init" => crate::commands::secrets::SecretsCommand::Init,
                "list" => crate::commands::secrets::SecretsCommand::List {
                    kind: secret_matches
                        .get_one::<String>("kind")
                        .map(|kind| parse_secret_kind(kind))
                        .transpose()?,
                },
                "add" => crate::commands::secrets::SecretsCommand::Add {
                    id: required_string(secret_matches, "id", "missing secret id")?,
                    kind: parse_secret_kind(
                        secret_matches
                            .get_one::<String>("kind")
                            .expect("required secret kind is present"),
                    )?,
                    label: secret_matches.get_one::<String>("label").cloned(),
                    description: secret_matches.get_one::<String>("description").cloned(),
                    open: secret_matches.get_flag("open"),
                },
                "edit" => crate::commands::secrets::SecretsCommand::Edit {
                    id: required_string(secret_matches, "id", "missing secret id")?,
                    label: secret_matches.get_one::<String>("label").cloned(),
                    description: secret_matches.get_one::<String>("description").cloned(),
                    open: secret_matches.get_flag("open"),
                },
                "remove" => crate::commands::secrets::SecretsCommand::Remove {
                    id: required_string(secret_matches, "id", "missing secret id")?,
                },
                _ => return Err(AppError::invalid_argument("unsupported secrets subcommand")),
            };
            RuntimeCommand::Secrets { request, options }
        }
        Some((HOST_COMMAND_UPGRADE, upgrade_matches)) => RuntimeCommand::Upgrade {
            request: crate::upgrade::route::request_from_matches(upgrade_matches)?,
            options,
        },
        Some((domain, domain_matches)) => {
            let mut argv = if domain == "run" {
                run_check_argv.unwrap_or(collect_domain_argv(domain_matches)?)
            } else {
                collect_domain_argv(domain_matches)?
            };
            let normalized =
                normalize_invocation_argv(&argv, options.to_wire()).map_err(|error| {
                    AppError::invalid_argument(
                        error
                            .error_message
                            .unwrap_or_else(|| "invalid invocation argument".to_owned()),
                    )
                })?;
            options = GlobalOptions::from(normalized.globals);
            argv = normalized.argv;
            RuntimeCommand::Invoke {
                domain: domain.to_owned(),
                argv,
                options,
            }
        }
        None => return Err(AppError::invalid_argument("missing command domain")),
    };

    Ok(CliParseResult::Command(runtime_command))
}

/// Any decision supplied on the command line disables prompting, so scripts and
/// pipes always take the documented defaults.
pub(super) fn has_decision_flags(matches: &ArgMatches, options: &GlobalOptions) -> bool {
    matches.contains_id("scope") && matches.get_one::<String>("scope").is_some()
        || matches.value_source("transport") == Some(ValueSource::CommandLine)
        || matches.get_one::<String>("url").is_some()
        || matches.get_flag("mcp-only")
        || matches.get_flag("rules-only")
        || matches.get_flag("yes")
        || matches.get_flag("dry-run")
        || options.output == OutputMode::Json
        || options.quiet
}

pub(super) fn required_target(matches: &ArgMatches) -> Result<String, AppError> {
    matches
        .get_one::<String>("target")
        .cloned()
        .ok_or_else(|| AppError::invalid_argument("missing agent target"))
}

pub(super) fn parse_scope(matches: &ArgMatches) -> Result<Option<Scope>, AppError> {
    matches
        .get_one::<String>("scope")
        .map(|value| {
            Scope::parse(value)
                .ok_or_else(|| AppError::invalid_argument(format!("unsupported scope: {value}")))
        })
        .transpose()
}

pub(super) fn parse_transport(matches: &ArgMatches) -> Result<Transport, AppError> {
    match matches
        .get_one::<String>("transport")
        .map(String::as_str)
        .unwrap_or("stdio")
    {
        "stdio" => Ok(Transport::Stdio),
        // `managed` is provisioning, not a wire format: the agent still gets an
        // HTTP entry, only the endpoint is one AIHelper owns.
        "http" | "managed" => Ok(Transport::Http),
        value => Err(AppError::invalid_argument(format!(
            "unsupported MCP transport: {value}"
        ))),
    }
}

pub(super) fn parse_plugin_state_filter(value: &str) -> Result<PluginStateFilter, AppError> {
    match value {
        "enabled" => Ok(PluginStateFilter::Enabled),
        "disabled" => Ok(PluginStateFilter::Disabled),
        _ => Err(AppError::invalid_argument(format!(
            "unsupported plugins --state value: {value}"
        ))),
    }
}

pub(super) fn parse_secret_kind(value: &str) -> Result<crate::secrets::SecretKind, AppError> {
    value
        .parse()
        .map_err(|()| AppError::invalid_argument(format!("unsupported secret kind: {value}")))
}

pub(super) fn required_string(
    matches: &ArgMatches,
    name: &str,
    error: &'static str,
) -> Result<String, AppError> {
    matches
        .get_one::<String>(name)
        .cloned()
        .ok_or_else(|| AppError::invalid_argument(error))
}

pub(super) fn collect_domain_argv(matches: &ArgMatches) -> Result<Vec<String>, AppError> {
    if let Ok(Some(values)) = matches.try_get_many::<String>("argv") {
        return Ok(values.cloned().collect());
    }
    if let Ok(Some(values)) = matches.try_get_many::<OsString>("") {
        return values
            .map(|value| {
                value.to_str().map(str::to_owned).ok_or_else(|| {
                    AppError::invalid_argument("external subcommand contains non-UTF8 argument")
                })
            })
            .collect();
    }
    Ok(Vec::new())
}
