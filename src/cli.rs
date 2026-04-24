use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use ah_plugin_api::{GlobalOptionsWire, PluginMetadata};
use clap::{
    Arg, ArgAction, ArgGroup, ArgMatches, Command, ValueHint, error::ErrorKind, value_parser,
};

use crate::{error::AppError, output::OutputMode};

const HOST_COMMAND_AI: &str = "ai";
const HOST_COMMAND_PLUGINS: &str = "plugins";

pub enum RuntimeCommand {
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
    Invoke {
        domain: String,
        argv: Vec<String>,
        options: GlobalOptions,
    },
}

pub enum CliParseResult {
    Command(RuntimeCommand),
    ExitSuccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStateFilter {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy)]
pub struct GlobalOptions {
    pub output: OutputMode,
    pub quiet: bool,
    pub limit: Option<usize>,
}

impl GlobalOptions {
    pub fn to_wire(self) -> GlobalOptionsWire {
        GlobalOptionsWire {
            json: self.output == OutputMode::Json,
            quiet: self.quiet,
            limit: self.limit,
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
        }
    }
}

pub fn apply_initial_cwd_from_raw_args(raw_args: &[OsString]) -> Result<(), AppError> {
    if let Some(cwd) = extract_last_cwd(raw_args)? {
        std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
    }
    Ok(())
}

pub fn parse_runtime_command(
    raw_args: Vec<OsString>,
    plugins: &[PluginMetadata],
) -> Result<CliParseResult, AppError> {
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
            _ => return Err(AppError::invalid_argument(error.to_string())),
        },
    };

    let mut options = GlobalOptions {
        output: if matches.get_flag("json") {
            OutputMode::Json
        } else {
            OutputMode::Text
        },
        quiet: matches.get_flag("quiet"),
        limit: matches.get_one::<usize>("limit").copied(),
    };
    if options.limit == Some(0) {
        return Err(AppError::invalid_argument("--limit must be >= 1"));
    }
    let mut deferred_cwd = matches.get_one::<PathBuf>("cwd").cloned();

    let runtime_command = match matches.subcommand() {
        Some((HOST_COMMAND_AI, ai_matches)) => {
            let Some((subcommand, ai_submatches)) = ai_matches.subcommand() else {
                return Err(AppError::invalid_argument("missing ai subcommand"));
            };
            match subcommand {
                "info" => RuntimeCommand::AiInfo {
                    domain: ai_submatches.get_one::<String>("domain").cloned(),
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
        Some((domain, domain_matches)) => {
            let mut argv = collect_domain_argv(domain_matches)?;
            argv = strip_trailing_global_flags(&argv, &mut options, &mut deferred_cwd)?;
            RuntimeCommand::Invoke {
                domain: domain.to_owned(),
                argv,
                options,
            }
        }
        None => return Err(AppError::invalid_argument("missing command domain")),
    };

    if let Some(cwd) = deferred_cwd {
        std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
    }

    Ok(CliParseResult::Command(runtime_command))
}

fn build_cli_command(plugins: &[PluginMetadata]) -> Command {
    let mut command = Command::new("ah")
        .version(env!("CARGO_PKG_VERSION"))
        .about("AIHelper CLI toolbox for AI agents and developers")
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Return machine-readable JSON output"),
        )
        .arg(
            Arg::new("quiet")
                .long("quiet")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Suppress command output"),
        )
        .arg(
            Arg::new("cwd")
                .long("cwd")
                .value_name("PATH")
                .value_hint(ValueHint::DirPath)
                .value_parser(value_parser!(PathBuf))
                .global(true)
                .help("Set working directory"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_name("N")
                .value_parser(value_parser!(usize))
                .global(true)
                .help("Cap output lines/items when supported"),
        )
        .subcommand(build_ai_command())
        .subcommand(build_plugins_command())
        .allow_external_subcommands(true);

    for (domain, description) in plugin_domains_for_help(plugins) {
        if domain == HOST_COMMAND_AI || domain == HOST_COMMAND_PLUGINS {
            continue;
        }
        command = command.subcommand(build_domain_command(&domain, &description));
    }

    command
}

fn build_ai_command() -> Command {
    Command::new(HOST_COMMAND_AI)
        .about("AI-agent focused command manual")
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

fn extract_last_cwd(raw_args: &[OsString]) -> Result<Option<PathBuf>, AppError> {
    let mut cwd = None;
    let mut index = 1usize;
    while index < raw_args.len() {
        let arg = &raw_args[index];
        if arg == "--cwd" {
            let Some(value) = raw_args.get(index + 1) else {
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

fn strip_trailing_global_flags(
    argv: &[String],
    options: &mut GlobalOptions,
    deferred_cwd: &mut Option<PathBuf>,
) -> Result<Vec<String>, AppError> {
    let mut filtered = Vec::new();
    let mut index = 0usize;
    while index < argv.len() {
        match argv[index].as_str() {
            "--json" => {
                options.output = OutputMode::Json;
                index += 1;
            }
            "--quiet" => {
                options.quiet = true;
                index += 1;
            }
            "--limit" => {
                let value = argv.get(index + 1).ok_or_else(|| {
                    AppError::invalid_argument("missing value for trailing --limit")
                })?;
                let parsed = value.parse::<usize>().map_err(|_| {
                    AppError::invalid_argument(format!(
                        "invalid value for trailing --limit: {value}"
                    ))
                })?;
                if parsed == 0 {
                    return Err(AppError::invalid_argument("--limit must be >= 1"));
                }
                options.limit = Some(parsed);
                index += 2;
            }
            "--cwd" => {
                let value = argv.get(index + 1).ok_or_else(|| {
                    AppError::invalid_argument("missing value for trailing --cwd")
                })?;
                *deferred_cwd = Some(PathBuf::from(value));
                index += 2;
            }
            _ => {
                if let Some(value) = argv[index].strip_prefix("--cwd=") {
                    *deferred_cwd = Some(PathBuf::from(value));
                } else if let Some(value) = argv[index].strip_prefix("--limit=") {
                    let parsed = value.parse::<usize>().map_err(|_| {
                        AppError::invalid_argument(format!(
                            "invalid value for trailing --limit: {value}"
                        ))
                    })?;
                    if parsed == 0 {
                        return Err(AppError::invalid_argument("--limit must be >= 1"));
                    }
                    options.limit = Some(parsed);
                } else {
                    filtered.push(argv[index].clone());
                }
                index += 1;
            }
        }
    }
    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_includes_dynamic_plugin_domain() {
        let plugins = vec![
            PluginMetadata {
                plugin_name: "builtin-file".to_owned(),
                domain: "file".to_owned(),
                description: "File operations plugin (built-in)".to_owned(),
                abi_version: 1,
            },
            PluginMetadata {
                plugin_name: "external-ollama".to_owned(),
                domain: "ollama".to_owned(),
                description: "Ollama Local API plugin (dynamic)".to_owned(),
                abi_version: 1,
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
    fn parser_parses_plugins_state_management_commands() {
        let plugins = vec![PluginMetadata {
            plugin_name: "builtin-http".to_owned(),
            domain: "http".to_owned(),
            description: "HTTP workflow plugin (built-in)".to_owned(),
            abi_version: 1,
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
