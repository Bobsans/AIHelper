use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use ah_plugin_api::{GlobalOptionsWire, PluginMetadata, normalize_invocation_argv};
use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches, Command, error::ErrorKind, parser::ValueSource,
    value_parser,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
    ai::{
        install::{AiCommand, InstallRequest, StatusRequest, UninstallRequest},
        targets::{Scope, Transport},
    },
    error::{AppError, CommandSuggestion, suggested_subcommand},
    output::OutputMode,
};

const HOST_COMMAND_AI: &str = "ai";
const HOST_COMMAND_MCP: &str = "mcp";
const HOST_COMMAND_PLUGINS: &str = "plugins";
const HOST_COMMAND_SECRETS: &str = "secrets";
const HOST_COMMAND_UPGRADE: &str = "upgrade";

pub enum RuntimeCommand {
    McpServe {
        transport: McpTransport,
        port: u16,
        max_active: usize,
        default_timeout_ms: u64,
        options: GlobalOptions,
    },
    PluginsList {
        state_filter: Option<PluginStateFilter>,
        options: GlobalOptions,
    },
    PluginsEnable {
        domain: String,
        options: GlobalOptions,
    },
    PluginsDisable {
        domain: String,
        options: GlobalOptions,
    },
    PluginsReset {
        domain: Option<String>,
        all: bool,
        options: GlobalOptions,
    },
    AiInfo {
        domain: Option<String>,
        options: GlobalOptions,
    },
    Ai {
        request: crate::ai::install::AiCommand,
        options: GlobalOptions,
    },
    Secrets {
        request: crate::commands::secrets::SecretsCommand,
        options: GlobalOptions,
    },
    Upgrade {
        request: crate::updater::command::UpgradeRequest,
        options: GlobalOptions,
    },
    Invoke {
        domain: String,
        argv: Vec<String>,
        options: GlobalOptions,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    Http,
}

pub enum CliParseResult {
    Command(RuntimeCommand),
    ExitSuccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PluginStateFilter {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone)]
pub struct GlobalOptions {
    pub output: OutputMode,
    pub quiet: bool,
    pub limit: Option<usize>,
    /// The directory this request resolves relative paths against.
    ///
    /// `None` means the process directory, which is what the shell handed us
    /// and is the right answer when `--cwd` was not given. It is read, never
    /// written: the previous mechanism was a process-wide `chdir` at startup,
    /// which made the answer global to a process that serves requests in
    /// parallel.
    pub cwd: Option<PathBuf>,
}

impl GlobalOptions {
    pub fn to_wire(&self) -> GlobalOptionsWire {
        GlobalOptionsWire {
            json: self.output == OutputMode::Json,
            quiet: self.quiet,
            limit: self.limit,
            cwd: self
                .cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
        }
    }
}

impl From<GlobalOptionsWire> for GlobalOptions {
    fn from(value: GlobalOptionsWire) -> Self {
        Self {
            output: if value.json {
                OutputMode::Json
            } else {
                OutputMode::Text
            },
            quiet: value.quiet,
            limit: value.limit,
            cwd: value.cwd.map(PathBuf::from),
        }
    }
}

/// The request directory named by `--cwd`, resolved against the process
/// directory so that later consumers never have to.
///
/// This used to call `std::env::set_current_dir`, which made the answer a
/// property of the process rather than of the request - and `mcp serve` runs
/// requests in parallel. The directory is now read here and carried.
///
/// # Errors
///
/// [`AppError`] when `--cwd` ends argv with no value, or names something that
/// is not a readable directory.
pub fn initial_cwd_from_raw_args(raw_args: &[OsString]) -> Result<Option<PathBuf>, AppError> {
    let Some(cwd) = extract_last_cwd(raw_args)? else {
        return Ok(None);
    };
    resolve_request_dir(cwd).map(Some)
}

/// Turn a `--cwd` value into the absolute directory consumers resolve against.
///
/// One rule, used by both the early resolution and the full parse, so the two
/// cannot disagree about what `--cwd` meant.
///
/// # Errors
///
/// [`AppError`] when the path is not a readable directory. The `chdir` this
/// replaces failed the same way, so a missing `--cwd` still fails rather than
/// being silently ignored.
pub fn resolve_request_dir(cwd: PathBuf) -> Result<PathBuf, AppError> {
    // Absolute, so a consumer joining a relative path onto it cannot fall back
    // to the process directory without saying so.
    let resolved = cwd
        .canonicalize()
        .map_err(|source| AppError::cwd(cwd.clone(), source))?;
    if !resolved.is_dir() {
        return Err(AppError::invalid_argument(format!(
            "--cwd is not a directory: {}",
            cwd.display()
        )));
    }
    Ok(resolved)
}

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
            request: crate::updater::command::request_from_matches(upgrade_matches)?,
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

pub(crate) fn redact_secret_command_argv(raw_args: &[OsString]) -> Vec<OsString> {
    let mut sanitized = raw_args.to_vec();
    let Some(action_index) = secret_action_index(&sanitized) else {
        return sanitized;
    };
    let add = sanitized[action_index] == "add";
    let mut id_seen = false;
    let mut preserve_next = false;
    let mut redact_next = false;
    let mut positional_only = false;

    for argument in sanitized.iter_mut().skip(action_index + 1) {
        let value = argument.to_string_lossy();
        if preserve_next {
            preserve_next = false;
            continue;
        }
        // An unknown option may carry a secret, so its argument is redacted even
        // when the value itself looks like an option.
        if redact_next {
            *argument = OsString::from(ah_redact::REDACTED);
            redact_next = false;
            continue;
        }

        if !positional_only && value == "--" {
            positional_only = true;
            continue;
        }
        if !positional_only && value.starts_with('-') {
            let body = value.trim_start_matches('-');
            let (name, assigned) = body
                .split_once('=')
                .map_or((body, false), |(name, _)| (name, true));
            let known_value =
                matches!(name, "label" | "description" | "cwd" | "limit") || add && name == "kind";
            if known_value {
                preserve_next = !assigned;
            } else if assigned {
                *argument = OsString::from(format!("--{name}={}", ah_redact::REDACTED));
            } else if !matches!(name, "open" | "json" | "quiet" | "help") {
                redact_next = true;
            }
            continue;
        }
        if id_seen {
            *argument = OsString::from(ah_redact::REDACTED);
        } else {
            id_seen = true;
        }
    }
    sanitized
}

fn secret_action_index(raw_args: &[OsString]) -> Option<usize> {
    let mut index = 1;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != HOST_COMMAND_SECRETS {
        return None;
    }
    index += 1;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        return matches!(raw_args[index].to_str(), Some("add" | "edit")).then_some(index);
    }
    None
}

fn decorate_cli_parse_error(error: AppError, command: &Command, raw_args: &[OsString]) -> AppError {
    let detail = error.detail_message();
    let Some(candidate) = suggested_subcommand(&detail) else {
        return error;
    };
    let mut scope = command;
    let mut index = 1usize;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        let Some(argument) = raw_args[index].to_str() else {
            break;
        };
        let Some(subcommand) = scope.find_subcommand(argument) else {
            break;
        };
        scope = subcommand;
        index += 1;
    }
    let description = scope
        .find_subcommand(candidate)
        .and_then(Command::get_about)
        .map(ToString::to_string);
    error.with_suggestion_context(description, None)
}

pub(crate) fn suggest_top_level_command(
    domain: &str,
    plugins: &[PluginMetadata],
) -> Option<CommandSuggestion> {
    let normalized = domain.to_ascii_lowercase();
    let aliases = [
        ("version", "ah --version", "Show the AIHelper version"),
        ("help", "ah --help", "Show command help"),
    ];
    let mut candidates = aliases
        .iter()
        .map(|(name, command, description)| {
            (
                (*name).to_owned(),
                (*command).to_owned(),
                (*description).to_owned(),
            )
        })
        .collect::<Vec<_>>();
    candidates.extend(
        [
            (HOST_COMMAND_AI, "AI-agent focused command manual"),
            (HOST_COMMAND_MCP, "Model Context Protocol server"),
            (HOST_COMMAND_PLUGINS, "Plugin management commands"),
            (HOST_COMMAND_SECRETS, "Encrypted secret vault management"),
            (
                HOST_COMMAND_UPGRADE,
                "Check for or install AIHelper updates",
            ),
        ]
        .into_iter()
        .map(|(name, description)| {
            (
                name.to_owned(),
                format!("ah {name}"),
                description.to_owned(),
            )
        }),
    );
    candidates.extend(plugins.iter().map(|plugin| {
        let name = plugin.domain.to_ascii_lowercase();
        (
            name.clone(),
            format!("ah {name}"),
            top_level_domain_summary(&name, &plugin.description),
        )
    }));
    candidates.sort();
    candidates.dedup_by(|left, right| left.0 == right.0);

    candidates
        .into_iter()
        .map(|(name, command, description)| {
            (
                edit_distance(&normalized, &name),
                name.len(),
                command,
                description,
            )
        })
        .filter(|(distance, candidate_len, _, _)| {
            *distance <= 2 && distance.saturating_mul(3) <= normalized.len().max(*candidate_len)
        })
        .min_by_key(|(distance, candidate_len, command, _)| {
            (
                *distance,
                normalized.len().abs_diff(*candidate_len),
                command.clone(),
            )
        })
        .map(|(_, _, command, description)| CommandSuggestion::new(command, Some(description)))
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(left_index + 1);
        for (right_index, right_char) in right.iter().enumerate() {
            let substitution = previous[right_index] + usize::from(left_char != *right_char);
            current.push(
                (current[right_index] + 1)
                    .min(previous[right_index + 1] + 1)
                    .min(substitution),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

pub(crate) fn build_cli_command(plugins: &[PluginMetadata]) -> Command {
    let mut command = crate::entry::with_global_flags(
        Command::new("ah")
            .version(env!("CARGO_PKG_VERSION"))
            .about("AIHelper CLI toolbox for AI agents and developers"),
    )
    .subcommand(build_ai_command())
    .subcommand(build_mcp_command())
    .subcommand(build_plugins_command())
    .subcommand(build_secrets_command())
    .subcommand(crate::updater::command::build_help_command())
    .allow_external_subcommands(true);

    for (domain, description) in plugin_domains_for_help(plugins) {
        if domain == HOST_COMMAND_AI
            || domain == HOST_COMMAND_MCP
            || domain == HOST_COMMAND_PLUGINS
            || domain == HOST_COMMAND_SECRETS
            || domain == HOST_COMMAND_UPGRADE
        {
            continue;
        }
        command = command.subcommand(build_domain_command(&domain, &description));
    }

    command
}

fn build_mcp_command() -> Command {
    Command::new(HOST_COMMAND_MCP)
        .about("Model Context Protocol server")
        .subcommand(
            Command::new("serve")
                .about("Serve typed AIHelper tools over MCP stdio or local HTTP")
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .value_name("TRANSPORT")
                        .value_parser(["stdio", "http"])
                        .default_value("stdio")
                        .help("MCP transport: stdio or local Streamable HTTP"),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .value_parser(value_parser!(u16))
                        .help("Local HTTP port (default: 8787; HTTP transport only)"),
                )
                .arg(
                    Arg::new("max-active")
                        .long("max-active")
                        .value_name("N")
                        .value_parser(value_parser!(usize))
                        .default_value("32")
                        .help("Maximum number of concurrently active command handlers"),
                )
                .arg(
                    Arg::new("max-queued")
                        .long("max-queued")
                        .value_name("N")
                        .value_parser(value_parser!(usize))
                        .hide(true),
                )
                .arg(
                    Arg::new("default-timeout-ms")
                        .long("default-timeout-ms")
                        .value_name("MILLISECONDS")
                        .value_parser(value_parser!(u64))
                        .default_value("300000")
                        .help("Default command execution timeout"),
                )
                .arg(crate::mcp_service::command::managed_config_arg()),
        )
        .subcommand(crate::mcp_service::command::build_service_help_command())
}

fn build_ai_command() -> Command {
    Command::new(HOST_COMMAND_AI)
        .about("AI-agent manual and agent integration")
        .subcommand(
            Command::new("info")
                .about("Show full AI-agent manual for available commands")
                .arg(
                    Arg::new("domain")
                        .long("domain")
                        .value_name("DOMAIN")
                        .value_parser(value_parser!(String))
                        .help("Show manual only for a single command domain"),
                ),
        )
        .subcommand(
            Command::new("install")
                .about("Register the AIHelper MCP server and rules block in an AI agent")
                .arg(target_arg(true))
                .arg(scope_arg())
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .value_name("TRANSPORT")
                        .value_parser(["stdio", "http", "managed"])
                        .default_value("stdio")
                        .help(
                            "MCP transport written into the agent configuration;                              `managed` uses the Windows managed service endpoint",
                        ),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .value_name("URL")
                        .value_parser(value_parser!(String))
                        .help("Loopback MCP endpoint; only valid with --transport http"),
                )
                .arg(
                    Arg::new("mcp-only")
                        .long("mcp-only")
                        .action(ArgAction::SetTrue)
                        .help("Register the MCP server without touching the rules file"),
                )
                .arg(
                    Arg::new("rules-only")
                        .long("rules-only")
                        .action(ArgAction::SetTrue)
                        .help("Install the rules block without registering the MCP server"),
                )
                .group(ArgGroup::new("ai-components").args(["mcp-only", "rules-only"]))
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .short('y')
                        .action(ArgAction::SetTrue)
                        .help("Skip the interactive confirmation prompt"),
                )
                .arg(dry_run_arg()),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove the AIHelper MCP server and rules block from an AI agent")
                .arg(target_arg(true))
                .arg(scope_arg())
                .arg(dry_run_arg()),
        )
        .subcommand(
            Command::new("status")
                .about("Report AIHelper integration state for known AI agents")
                .arg(target_arg(false)),
        )
}

fn target_arg(required: bool) -> Arg {
    Arg::new("target")
        .value_name("TARGET")
        .required(required)
        .value_parser(value_parser!(String))
        .help("Agent target, for example claude or codex")
}

fn scope_arg() -> Arg {
    Arg::new("scope")
        .long("scope")
        .value_name("SCOPE")
        .value_parser(["local", "project", "user"])
        .help("Configuration scope understood by the target agent")
}

fn dry_run_arg() -> Arg {
    Arg::new("dry-run")
        .long("dry-run")
        .action(ArgAction::SetTrue)
        .help("Report planned commands and file changes without performing them")
}

/// Any decision supplied on the command line disables prompting, so scripts and
/// pipes always take the documented defaults.
fn has_decision_flags(matches: &ArgMatches, options: &GlobalOptions) -> bool {
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

fn required_target(matches: &ArgMatches) -> Result<String, AppError> {
    matches
        .get_one::<String>("target")
        .cloned()
        .ok_or_else(|| AppError::invalid_argument("missing agent target"))
}

fn parse_scope(matches: &ArgMatches) -> Result<Option<Scope>, AppError> {
    matches
        .get_one::<String>("scope")
        .map(|value| {
            Scope::parse(value)
                .ok_or_else(|| AppError::invalid_argument(format!("unsupported scope: {value}")))
        })
        .transpose()
}

fn parse_transport(matches: &ArgMatches) -> Result<Transport, AppError> {
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

fn build_plugins_command() -> Command {
    Command::new(HOST_COMMAND_PLUGINS)
        .about("Plugin management commands")
        .subcommand(
            Command::new("list").about("List registered plugins").arg(
                Arg::new("state")
                    .long("state")
                    .value_name("STATE")
                    .value_parser(["enabled", "disabled"])
                    .help("Filter by plugin domain state"),
            ),
        )
        .subcommand(
            Command::new("enable")
                .about("Enable plugin domain")
                .arg(Arg::new("domain").value_name("DOMAIN").required(true)),
        )
        .subcommand(
            Command::new("disable")
                .about("Disable plugin domain")
                .arg(Arg::new("domain").value_name("DOMAIN").required(true)),
        )
        .subcommand(
            Command::new("reset")
                .about("Reset plugin domain override")
                .arg(Arg::new("domain").value_name("DOMAIN").required(false))
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Reset all domain overrides"),
                )
                .group(
                    ArgGroup::new("plugins-reset-target")
                        .args(["domain", "all"])
                        .required(true)
                        .multiple(false),
                ),
        )
}

fn build_secrets_command() -> Command {
    let metadata_args = || {
        [
            Arg::new("label")
                .long("label")
                .value_name("TEXT")
                .help("Human-readable secret label"),
            Arg::new("description")
                .long("description")
                .value_name("TEXT")
                .help("Optional secret description"),
        ]
    };
    Command::new(HOST_COMMAND_SECRETS)
        .about("Encrypted secret vault management")
        .subcommand(Command::new("init").about("Initialize the encrypted secret vault"))
        .subcommand(
            Command::new("list")
                .about("List redacted secret metadata")
                .arg(
                    Arg::new("kind")
                        .long("kind")
                        .value_name("KIND")
                        .value_parser(secret_kind_values())
                        .help("Filter by secret kind"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a secret using hidden prompts")
                .arg(Arg::new("id").value_name("ID").required(true))
                .arg(
                    Arg::new("kind")
                        .long("kind")
                        .value_name("KIND")
                        .value_parser(secret_kind_values())
                        .required(true),
                )
                .arg(
                    Arg::new("open")
                        .long("open")
                        .action(ArgAction::SetTrue)
                        .help("Open a protected browser setup form"),
                )
                .args(metadata_args()),
        )
        .subcommand(
            Command::new("edit")
                .about("Edit a secret using hidden prompts")
                .arg(Arg::new("id").value_name("ID").required(true))
                .arg(
                    Arg::new("open")
                        .long("open")
                        .action(ArgAction::SetTrue)
                        .help("Open a protected browser setup form"),
                )
                .args(metadata_args()),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove a secret")
                .arg(Arg::new("id").value_name("ID").required(true)),
        )
}

fn build_domain_command(domain: &str, description: &str) -> Command {
    Command::new(domain.to_owned())
        .about(description.to_owned())
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("argv")
                .num_args(0..)
                .action(ArgAction::Append)
                .allow_hyphen_values(true)
                .trailing_var_arg(true)
                .value_parser(value_parser!(String)),
        )
}

fn plugin_domains_for_help(plugins: &[PluginMetadata]) -> Vec<(String, String)> {
    let mut by_domain = BTreeMap::new();
    for plugin in plugins {
        by_domain.insert(
            plugin.domain.clone(),
            top_level_domain_summary(&plugin.domain, &plugin.description),
        );
    }
    by_domain.into_iter().collect()
}

fn top_level_domain_summary(domain: &str, fallback: &str) -> String {
    match domain {
        "file" => "File utilities".to_owned(),
        "search" => "Search utilities".to_owned(),
        "ctx" => "Context-reduction utilities".to_owned(),
        "git" => "Git-focused utilities".to_owned(),
        "project" => "Project detection utilities".to_owned(),
        "run" => "Command execution check utilities".to_owned(),
        "http" => "HTTP workflow utilities".to_owned(),
        "task" => "Task recipe utilities".to_owned(),
        _ => fallback.to_owned(),
    }
}

fn parse_plugin_state_filter(value: &str) -> Result<PluginStateFilter, AppError> {
    match value {
        "enabled" => Ok(PluginStateFilter::Enabled),
        "disabled" => Ok(PluginStateFilter::Disabled),
        _ => Err(AppError::invalid_argument(format!(
            "unsupported plugins --state value: {value}"
        ))),
    }
}

fn secret_kind_values() -> Vec<&'static str> {
    crate::secrets::SecretKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect()
}

fn parse_secret_kind(value: &str) -> Result<crate::secrets::SecretKind, AppError> {
    value
        .parse()
        .map_err(|()| AppError::invalid_argument(format!("unsupported secret kind: {value}")))
}

fn required_string(
    matches: &ArgMatches,
    name: &str,
    error: &'static str,
) -> Result<String, AppError> {
    matches
        .get_one::<String>(name)
        .cloned()
        .ok_or_else(|| AppError::invalid_argument(error))
}

fn collect_domain_argv(matches: &ArgMatches) -> Result<Vec<String>, AppError> {
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

#[derive(Debug, Clone, Copy)]
struct RunCheckLayout {
    domain_index: usize,
    prefix_end: usize,
    child_start: usize,
}

fn prepare_run_check_passthrough(
    raw_args: &mut [OsString],
) -> Result<Option<Vec<String>>, AppError> {
    let Some(layout) = run_check_layout(raw_args) else {
        return Ok(None);
    };

    let mut argv = os_args_to_strings(&raw_args[(layout.domain_index + 1)..layout.prefix_end])?;
    argv.push("--".to_owned());
    argv.extend(os_args_to_strings(&raw_args[layout.child_start..])?);

    for (offset, value) in raw_args[layout.child_start..].iter_mut().enumerate() {
        *value = OsString::from(format!("__ah_opaque_child_arg_{offset}__"));
    }

    Ok(Some(argv))
}

fn os_args_to_strings(values: &[OsString]) -> Result<Vec<String>, AppError> {
    values
        .iter()
        .map(|value| {
            value.to_str().map(str::to_owned).ok_or_else(|| {
                AppError::invalid_argument("external subcommand contains non-UTF8 argument")
            })
        })
        .collect()
}

fn run_check_layout(raw_args: &[OsString]) -> Option<RunCheckLayout> {
    let mut index = 1usize;
    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != "run" {
        return None;
    }
    let domain_index = index;
    index += 1;

    while index < raw_args.len() {
        if let Some(next) = host_option_end(raw_args, index) {
            index = next;
            continue;
        }
        break;
    }
    if raw_args.get(index)? != "check" {
        return None;
    }
    index += 1;

    while index < raw_args.len() {
        if raw_args[index] == "--" {
            return Some(RunCheckLayout {
                domain_index,
                prefix_end: index,
                child_start: index + 1,
            });
        }
        if let Some(next) =
            host_option_end(raw_args, index).or_else(|| run_check_option_end(raw_args, index))
        {
            index = next;
            continue;
        }
        return Some(RunCheckLayout {
            domain_index,
            prefix_end: index,
            child_start: index,
        });
    }

    None
}

fn host_option_end(raw_args: &[OsString], index: usize) -> Option<usize> {
    let value = raw_args.get(index)?.to_str()?;
    match value {
        "--json" | "--quiet" => Some(index + 1),
        "--cwd" | "--limit" => Some((index + 2).min(raw_args.len())),
        _ if value.starts_with("--cwd=") || value.starts_with("--limit=") => Some(index + 1),
        _ => None,
    }
}

fn run_check_option_end(raw_args: &[OsString], index: usize) -> Option<usize> {
    let value = raw_args.get(index)?.to_str()?;
    match value {
        "--timeout-secs" | "--max-output-bytes" | "--tail-lines" => {
            Some((index + 2).min(raw_args.len()))
        }
        _ if value.starts_with("--timeout-secs=")
            || value.starts_with("--max-output-bytes=")
            || value.starts_with("--tail-lines=") =>
        {
            Some(index + 1)
        }
        _ => None,
    }
}

fn extract_last_cwd(raw_args: &[OsString]) -> Result<Option<PathBuf>, AppError> {
    let mut cwd = None;
    let mut index = 1usize;
    let scan_end = run_check_layout(raw_args)
        .map(|layout| layout.prefix_end)
        .unwrap_or(raw_args.len());
    while index < scan_end {
        let arg = &raw_args[index];
        if arg == "--" {
            break;
        }
        if arg == "--cwd" {
            let Some(value) = raw_args.get(index + 1).filter(|_| index + 1 < scan_end) else {
                return Err(AppError::invalid_argument(
                    "missing value for trailing --cwd",
                ));
            };
            cwd = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        if let Some(value) = arg.to_str().and_then(|raw| raw.strip_prefix("--cwd=")) {
            cwd = Some(PathBuf::from(value));
        }
        index += 1;
    }
    Ok(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_matches(args: &[&str]) -> ArgMatches {
        let mut argv = vec!["ai", "install", "claude"];
        argv.extend_from_slice(args);
        build_ai_command()
            .try_get_matches_from(argv)
            .expect("ai install arguments should parse")
            .subcommand_matches("install")
            .expect("install subcommand should match")
            .clone()
    }

    fn text_options() -> GlobalOptions {
        GlobalOptions {
            output: OutputMode::Text,
            quiet: false,
            limit: None,
            cwd: None,
        }
    }

    #[test]
    fn a_bare_install_carries_no_decision_flags() {
        assert!(!has_decision_flags(&install_matches(&[]), &text_options()));
    }

    #[test]
    fn every_decision_flag_disables_prompting() {
        for args in [
            vec!["--scope", "user"],
            vec!["--transport", "http"],
            vec!["--transport", "stdio"],
            vec!["--mcp-only"],
            vec!["--rules-only"],
            vec!["--dry-run"],
            vec!["--yes"],
        ] {
            assert!(
                has_decision_flags(&install_matches(&args), &text_options()),
                "{args:?} must disable prompting"
            );
        }
    }

    #[test]
    fn machine_readable_and_quiet_output_also_disable_prompting() {
        let json = GlobalOptions {
            output: OutputMode::Json,
            quiet: false,
            limit: None,
            cwd: None,
        };
        let quiet = GlobalOptions {
            output: OutputMode::Text,
            quiet: true,
            limit: None,
            cwd: None,
        };
        assert!(has_decision_flags(&install_matches(&[]), &json));
        assert!(has_decision_flags(&install_matches(&[]), &quiet));
    }

    #[test]
    fn an_explicit_stdio_transport_is_distinguished_from_the_default() {
        assert_eq!(
            install_matches(&[]).value_source("transport"),
            Some(ValueSource::DefaultValue),
            "the default must not look like an explicit choice"
        );
        assert_eq!(
            install_matches(&["--transport", "stdio"]).value_source("transport"),
            Some(ValueSource::CommandLine)
        );
    }

    #[test]
    fn managed_transport_sets_the_provisioning_flag_and_stays_http() {
        let raw_args = [
            OsString::from("ah"),
            OsString::from("ai"),
            OsString::from("install"),
            OsString::from("claude"),
            OsString::from("--transport"),
            OsString::from("managed"),
        ]
        .to_vec();
        let parsed = parse_runtime_command(raw_args, &[]).expect("managed transport should parse");
        let CliParseResult::Command(RuntimeCommand::Ai {
            request: crate::ai::install::AiCommand::Install(request),
            ..
        }) = parsed
        else {
            panic!("expected an ai install command");
        };
        assert!(request.managed);
        assert_eq!(request.transport, Transport::Http);
    }

    #[test]
    fn secrets_redaction_skips_global_options_while_locating_action() {
        let raw_args = [
            "ah",
            "--cwd",
            "workspace",
            "secrets",
            "--limit",
            "10",
            "add",
            "billing",
            "--kind",
            "postgres",
            "hunter2",
        ]
        .map(OsString::from);

        let sanitized = redact_secret_command_argv(&raw_args);

        assert_eq!(sanitized.last(), Some(&OsString::from("[REDACTED]")));
        assert_eq!(sanitized[2], "workspace");
        assert_eq!(sanitized[5], "10");
    }

    #[test]
    fn secrets_redaction_covers_values_that_look_like_options() {
        let raw_args = [
            "ah",
            "secrets",
            "add",
            "billing",
            "--kind",
            "postgres",
            "--label",
            "Billing",
            "--open",
            "--password",
            "-hunter2",
        ]
        .map(OsString::from);

        let sanitized = redact_secret_command_argv(&raw_args);

        assert!(!sanitized.iter().any(|value| value == "-hunter2"));
        assert_eq!(sanitized.last(), Some(&OsString::from("[REDACTED]")));
        // Known metadata options and their values stay readable in the log.
        assert_eq!(sanitized[7], "Billing");
        assert_eq!(sanitized[8], "--open");
    }

    #[test]
    fn help_includes_dynamic_plugin_domain() {
        let plugins = vec![
            PluginMetadata {
                plugin_name: "builtin-file".to_owned(),
                domain: "file".to_owned(),
                description: "File operations plugin (built-in)".to_owned(),
                abi_version: 1,
                required_tools: Vec::new(),
                compatibility: Default::default(),
            },
            PluginMetadata {
                plugin_name: "external-ollama".to_owned(),
                domain: "ollama".to_owned(),
                description: "Ollama Local API plugin (dynamic)".to_owned(),
                abi_version: 1,
                required_tools: Vec::new(),
                compatibility: Default::default(),
            },
        ];
        let mut command = build_cli_command(&plugins);
        let mut out = Vec::new();
        command
            .write_long_help(&mut out)
            .expect("long help should render");
        let help_text = String::from_utf8(out).expect("help must be valid utf8");
        assert!(help_text.contains("file"));
        assert!(help_text.contains("ollama"));
        assert!(help_text.contains("Ollama Local API plugin (dynamic)"));
    }

    #[test]
    fn parser_routes_dynamic_domain_to_invoke() {
        let plugins = vec![PluginMetadata {
            plugin_name: "external-ollama".to_owned(),
            domain: "ollama".to_owned(),
            description: "Ollama Local API plugin (dynamic)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }];
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("ollama"),
            OsString::from("ask"),
            OsString::from("--model"),
            OsString::from("llama3.2"),
            OsString::from("--prompt"),
            OsString::from("ping"),
        ];
        let parsed = parse_runtime_command(raw_args, &plugins).expect("parse should succeed");
        let CliParseResult::Command(RuntimeCommand::Invoke { domain, argv, .. }) = parsed else {
            panic!("unexpected parse result")
        };
        assert_eq!(domain, "ollama");
        assert_eq!(argv, vec!["ask", "--model", "llama3.2", "--prompt", "ping"]);
    }

    #[test]
    fn parser_routes_upgrade_check_as_a_host_command() {
        let parsed = parse_runtime_command(
            vec![
                OsString::from("ah"),
                OsString::from("upgrade"),
                OsString::from("--check"),
            ],
            &[],
        )
        .unwrap();
        let CliParseResult::Command(RuntimeCommand::Upgrade { request, .. }) = parsed else {
            panic!("unexpected parse result")
        };
        assert_eq!(request, crate::updater::command::UpgradeRequest::Check);
    }

    #[test]
    fn parser_routes_secrets_add_metadata_without_values() {
        let parsed = parse_runtime_command(
            vec![
                OsString::from("ah"),
                OsString::from("secrets"),
                OsString::from("add"),
                OsString::from("billing"),
                OsString::from("--kind"),
                OsString::from("postgres"),
                OsString::from("--label"),
                OsString::from("Billing"),
                OsString::from("--description"),
                OsString::from("Production billing database"),
                OsString::from("--open"),
            ],
            &[],
        )
        .unwrap();
        let CliParseResult::Command(RuntimeCommand::Secrets { request, .. }) = parsed else {
            panic!("unexpected parse result")
        };
        let crate::commands::secrets::SecretsCommand::Add {
            id,
            kind,
            label,
            description,
            open,
        } = request
        else {
            panic!("unexpected secrets command")
        };
        assert_eq!(id, "billing");
        assert_eq!(kind, crate::secrets::SecretKind::Postgres);
        assert_eq!(label.as_deref(), Some("Billing"));
        assert_eq!(description.as_deref(), Some("Production billing database"));
        assert!(open);
    }

    #[test]
    fn parser_rejects_secrets_values_in_argv() {
        let secret = "must-not-enter-runtime-command";
        let error = parse_runtime_command(
            vec![
                OsString::from("ah"),
                OsString::from("secrets"),
                OsString::from("add"),
                OsString::from("billing"),
                OsString::from("--kind"),
                OsString::from("postgres"),
                OsString::from("--password"),
                OsString::from(secret),
            ],
            &[],
        )
        .err()
        .expect("secret argv values must be rejected");
        assert!(!error.detail_message().contains(secret));
    }

    #[test]
    fn parser_strips_invocation_globals_from_domain_argv() {
        let plugins = vec![PluginMetadata {
            plugin_name: "external-ollama".to_owned(),
            domain: "ollama".to_owned(),
            description: "Ollama Local API plugin (dynamic)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }];
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("ollama"),
            OsString::from("ask"),
            OsString::from("--json"),
            OsString::from("--quiet"),
            OsString::from("--limit"),
            OsString::from("3"),
            OsString::from("--prompt"),
            OsString::from("ping"),
        ];
        let parsed = parse_runtime_command(raw_args, &plugins).expect("parse should succeed");
        let CliParseResult::Command(RuntimeCommand::Invoke {
            domain,
            argv,
            options,
        }) = parsed
        else {
            panic!("unexpected parse result");
        };
        assert_eq!(domain, "ollama");
        assert_eq!(options.output, OutputMode::Json);
        assert!(options.quiet);
        assert_eq!(options.limit, Some(3));
        assert_eq!(argv, vec!["ask", "--prompt", "ping"]);
    }

    #[test]
    fn parser_routes_mcp_stdio_server_with_defaults() {
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("mcp"),
            OsString::from("serve"),
        ];
        let parsed = parse_runtime_command(raw_args, &[]).expect("mcp serve should parse");
        let CliParseResult::Command(RuntimeCommand::McpServe {
            transport,
            port,
            max_active,
            default_timeout_ms,
            options,
        }) = parsed
        else {
            panic!("unexpected parse result");
        };
        assert_eq!(transport, McpTransport::Stdio);
        assert_eq!(port, 8787);
        assert_eq!(max_active, 32);
        assert_eq!(default_timeout_ms, 300_000);
        assert_eq!(options.limit, None);
        assert!(!options.quiet);
    }

    #[test]
    fn parser_routes_mcp_http_server() {
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("mcp"),
            OsString::from("serve"),
            OsString::from("--transport"),
            OsString::from("http"),
            OsString::from("--port"),
            OsString::from("9123"),
            OsString::from("--max-active"),
            OsString::from("7"),
        ];
        let parsed = parse_runtime_command(raw_args, &[]).expect("HTTP MCP serve should parse");
        let CliParseResult::Command(RuntimeCommand::McpServe {
            transport,
            port,
            max_active,
            ..
        }) = parsed
        else {
            panic!("unexpected parse result");
        };
        assert_eq!(transport, McpTransport::Http);
        assert_eq!(port, 9123);
        assert_eq!(max_active, 7);
    }

    #[test]
    fn parser_rejects_removed_max_queued_with_migration_hint() {
        for raw_args in [
            vec![
                OsString::from("ah"),
                OsString::from("mcp"),
                OsString::from("serve"),
                OsString::from("--max-queued"),
                OsString::from("2"),
            ],
            vec![
                OsString::from("ah"),
                OsString::from("mcp"),
                OsString::from("serve"),
                OsString::from("--max-queued=2"),
            ],
        ] {
            let error = parse_runtime_command(raw_args, &[])
                .err()
                .expect("removed option must fail");
            assert!(error.detail_message().contains("--max-active"));
        }
    }

    #[test]
    fn parser_forwards_max_queued_to_plugin() {
        let plugins = vec![PluginMetadata {
            plugin_name: "external-ollama".to_owned(),
            domain: "ollama".to_owned(),
            description: "Ollama Local API plugin (dynamic)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }];
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("ollama"),
            OsString::from("ask"),
            OsString::from("--max-queued"),
            OsString::from("2"),
        ];

        let parsed = parse_runtime_command(raw_args, &plugins).expect("plugin args should parse");
        let CliParseResult::Command(RuntimeCommand::Invoke { argv, .. }) = parsed else {
            panic!("unexpected parse result");
        };
        assert_eq!(argv, vec!["ask", "--max-queued", "2"]);
    }

    #[test]
    fn parser_forwards_max_queued_to_run_check_child() {
        let plugins = vec![PluginMetadata {
            plugin_name: "builtin-run".to_owned(),
            domain: "run".to_owned(),
            description: "Command execution check utilities".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }];
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("run"),
            OsString::from("check"),
            OsString::from("child"),
            OsString::from("--max-queued"),
            OsString::from("2"),
        ];

        let parsed = parse_runtime_command(raw_args, &plugins).expect("child args should parse");
        let CliParseResult::Command(RuntimeCommand::Invoke { argv, .. }) = parsed else {
            panic!("unexpected parse result");
        };
        assert_eq!(argv, vec!["check", "--", "child", "--max-queued", "2"]);
    }

    #[test]
    fn parser_rejects_json_for_mcp_stdio_server() {
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("--json"),
            OsString::from("mcp"),
            OsString::from("serve"),
        ];
        let error = parse_runtime_command(raw_args, &[])
            .err()
            .expect("--json must be rejected");
        assert!(
            error
                .detail_message()
                .contains("stdout is the MCP transport")
        );
    }

    #[test]
    fn extract_last_cwd_supports_equals_form() {
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("file"),
            OsString::from("read"),
            OsString::from("a.txt"),
            OsString::from("--cwd=tmp/workdir"),
        ];
        let cwd = extract_last_cwd(&raw_args).expect("cwd extraction should succeed");
        assert_eq!(cwd, Some(PathBuf::from("tmp/workdir")));
    }

    #[test]
    fn extract_last_cwd_ignores_run_check_child_arguments() {
        let raw_args = vec![
            OsString::from("ah"),
            OsString::from("--cwd=workspace"),
            OsString::from("run"),
            OsString::from("check"),
            OsString::from("child"),
            OsString::from("--cwd"),
            OsString::from("nested"),
        ];

        let cwd = extract_last_cwd(&raw_args).expect("cwd extraction should succeed");
        assert_eq!(cwd, Some(PathBuf::from("workspace")));
    }

    #[test]
    fn prepare_run_check_preserves_opaque_child_suffix() {
        let mut raw_args = vec![
            OsString::from("ah"),
            OsString::from("--json"),
            OsString::from("run"),
            OsString::from("check"),
            OsString::from("--timeout-secs"),
            OsString::from("5"),
            OsString::from("child"),
            OsString::from("--json"),
            OsString::from("--limit"),
            OsString::from("not-a-host-limit"),
            OsString::from("--cwd"),
            OsString::from("nested"),
        ];

        let argv = prepare_run_check_passthrough(&mut raw_args)
            .expect("passthrough preparation should succeed")
            .expect("run check should be detected");
        assert_eq!(
            argv,
            vec![
                "check",
                "--timeout-secs",
                "5",
                "--",
                "child",
                "--json",
                "--limit",
                "not-a-host-limit",
                "--cwd",
                "nested",
            ]
        );
        assert_eq!(raw_args[6], "__ah_opaque_child_arg_0__");
        assert_eq!(raw_args[11], "__ah_opaque_child_arg_5__");
    }

    #[test]
    fn parser_parses_plugins_state_management_commands() {
        let plugins = vec![PluginMetadata {
            plugin_name: "builtin-http".to_owned(),
            domain: "http".to_owned(),
            description: "HTTP workflow plugin (built-in)".to_owned(),
            abi_version: 1,
            required_tools: Vec::new(),
            compatibility: Default::default(),
        }];

        let disable_args = vec![
            OsString::from("ah"),
            OsString::from("plugins"),
            OsString::from("disable"),
            OsString::from("http"),
        ];
        let parsed_disable =
            parse_runtime_command(disable_args, &plugins).expect("disable should parse");
        let CliParseResult::Command(RuntimeCommand::PluginsDisable { domain, .. }) = parsed_disable
        else {
            panic!("unexpected disable parse result");
        };
        assert_eq!(domain, "http");

        let list_args = vec![
            OsString::from("ah"),
            OsString::from("plugins"),
            OsString::from("list"),
            OsString::from("--state"),
            OsString::from("disabled"),
        ];
        let parsed_list = parse_runtime_command(list_args, &plugins).expect("list should parse");
        let CliParseResult::Command(RuntimeCommand::PluginsList { state_filter, .. }) = parsed_list
        else {
            panic!("unexpected list parse result");
        };
        assert_eq!(state_filter, Some(PluginStateFilter::Disabled));

        let reset_args = vec![
            OsString::from("ah"),
            OsString::from("plugins"),
            OsString::from("reset"),
            OsString::from("--all"),
        ];
        let parsed_reset =
            parse_runtime_command(reset_args, &plugins).expect("reset --all should parse");
        let CliParseResult::Command(RuntimeCommand::PluginsReset { all, domain, .. }) =
            parsed_reset
        else {
            panic!("unexpected reset parse result");
        };
        assert!(all);
        assert_eq!(domain, None);
    }
}
