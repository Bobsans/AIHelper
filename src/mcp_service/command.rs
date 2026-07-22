use std::{ffi::OsString, path::PathBuf};

use clap::{Arg, ArgAction, Command, ValueHint, error::ErrorKind, value_parser};

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

use super::model::{DEFAULT_MAX_ACTIVE, DEFAULT_PORT, DEFAULT_TIMEOUT_MS};

#[derive(Debug)]
pub enum EarlyRoute {
    NotManaged,
    ExitSuccess,
    Service(ServiceCommand),
    ManagedServe { definition_path: PathBuf },
}

#[derive(Debug)]
pub enum ServiceCommand {
    Install(InstallOptions),
    Start { options: GlobalOptions },
    Stop { options: GlobalOptions },
    Restart { options: GlobalOptions },
    Status { options: GlobalOptions },
    Uninstall { options: GlobalOptions },
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub no_start: bool,
    pub port: u16,
    pub max_active: usize,
    pub default_timeout_ms: u64,
    pub options: GlobalOptions,
}

pub fn route(raw_args: &[OsString]) -> Result<EarlyRoute, AppError> {
    let Some((domain, operation)) = command_tokens(raw_args)? else {
        return Ok(EarlyRoute::NotManaged);
    };
    if domain != "mcp" {
        return Ok(EarlyRoute::NotManaged);
    }
    let is_service = operation.as_deref() == Some("service");
    let is_managed_serve = operation.as_deref() == Some("serve")
        && raw_args.iter().any(|arg| {
            arg == "--managed-config"
                || arg
                    .to_str()
                    .is_some_and(|value| value.starts_with("--managed-config="))
        });
    if !is_service && !is_managed_serve {
        return Ok(EarlyRoute::NotManaged);
    }

    let matches = match build_early_command().try_get_matches_from(raw_args.iter().cloned()) {
        Ok(matches) => matches,
        Err(error) => match error.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                error
                    .print()
                    .map_err(|source| AppError::invalid_argument(source.to_string()))?;
                return Ok(EarlyRoute::ExitSuccess);
            }
            _ => return Err(AppError::invalid_argument(error.to_string())),
        },
    };
    let options = GlobalOptions {
        output: if matches.get_flag("json") {
            OutputMode::Json
        } else {
            OutputMode::Text
        },
        quiet: matches.get_flag("quiet"),
        limit: matches.get_one::<usize>("limit").copied(),
    };
    let Some(("mcp", mcp)) = matches.subcommand() else {
        return Ok(EarlyRoute::NotManaged);
    };
    match mcp.subcommand() {
        Some(("serve", serve)) => {
            if serve
                .get_one::<String>("transport")
                .is_some_and(|transport| transport != "http")
            {
                return Err(AppError::invalid_argument(
                    "--managed-config requires --transport http",
                ));
            }
            let definition_path = serve
                .get_one::<PathBuf>("managed-config")
                .cloned()
                .ok_or_else(|| AppError::invalid_argument("missing --managed-config value"))?;
            Ok(EarlyRoute::ManagedServe { definition_path })
        }
        Some(("service", service)) => match service.subcommand() {
            Some(("install", install)) => Ok(EarlyRoute::Service(ServiceCommand::Install(
                InstallOptions {
                    no_start: install.get_flag("no-start"),
                    port: install
                        .get_one::<u16>("port")
                        .copied()
                        .unwrap_or(DEFAULT_PORT),
                    max_active: install
                        .get_one::<usize>("max-active")
                        .copied()
                        .unwrap_or(DEFAULT_MAX_ACTIVE),
                    default_timeout_ms: install
                        .get_one::<u64>("default-timeout-ms")
                        .copied()
                        .unwrap_or(DEFAULT_TIMEOUT_MS),
                    options,
                },
            ))),
            Some(("start", _)) => Ok(EarlyRoute::Service(ServiceCommand::Start { options })),
            Some(("stop", _)) => Ok(EarlyRoute::Service(ServiceCommand::Stop { options })),
            Some(("restart", _)) => Ok(EarlyRoute::Service(ServiceCommand::Restart { options })),
            Some(("status", _)) => Ok(EarlyRoute::Service(ServiceCommand::Status { options })),
            Some(("uninstall", _)) => {
                Ok(EarlyRoute::Service(ServiceCommand::Uninstall { options }))
            }
            _ => Err(AppError::invalid_argument(
                "missing or unsupported mcp service subcommand",
            )),
        },
        _ => Ok(EarlyRoute::NotManaged),
    }
}

pub fn build_service_help_command() -> Command {
    service_command()
}

pub fn managed_config_arg() -> Arg {
    Arg::new("managed-config")
        .long("managed-config")
        .value_name("PATH")
        .value_hint(ValueHint::FilePath)
        .value_parser(value_parser!(PathBuf))
        .hide(true)
}

fn build_early_command() -> Command {
    Command::new("ah")
        .disable_version_flag(true)
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
        .subcommand(
            Command::new("mcp")
                .subcommand(service_command())
                .subcommand(
                    Command::new("serve")
                        .arg(
                            Arg::new("transport")
                                .long("transport")
                                .value_parser(["stdio", "http"])
                                .default_value("stdio"),
                        )
                        .arg(managed_config_arg()),
                ),
        )
}

fn service_command() -> Command {
    Command::new("service")
        .about("Manage the per-user Windows MCP service")
        .subcommand(
            Command::new("install")
                .about("Install or reconcile the managed MCP service")
                .arg(
                    Arg::new("no-start")
                        .long("no-start")
                        .action(ArgAction::SetTrue)
                        .help("Register the service without starting it"),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .value_parser(value_parser!(u16))
                        .default_value("8787"),
                )
                .arg(
                    Arg::new("max-active")
                        .long("max-active")
                        .value_name("N")
                        .value_parser(value_parser!(usize))
                        .default_value("32"),
                )
                .arg(
                    Arg::new("default-timeout-ms")
                        .long("default-timeout-ms")
                        .value_name("MILLISECONDS")
                        .value_parser(value_parser!(u64))
                        .default_value("300000"),
                ),
        )
        .subcommand(Command::new("start").about("Start the managed MCP service"))
        .subcommand(Command::new("stop").about("Stop the managed MCP service"))
        .subcommand(Command::new("restart").about("Restart the managed MCP service"))
        .subcommand(Command::new("status").about("Inspect managed MCP service state"))
        .subcommand(Command::new("uninstall").about("Uninstall the managed MCP service"))
}

fn command_tokens(raw_args: &[OsString]) -> Result<Option<(String, Option<String>)>, AppError> {
    let mut tokens = Vec::new();
    let mut index = 1;
    while index < raw_args.len() {
        let value = raw_args[index]
            .to_str()
            .ok_or_else(|| AppError::invalid_argument("command arguments must be valid Unicode"))?;
        if matches!(value, "--json" | "--quiet") {
            index += 1;
            continue;
        }
        if matches!(value, "--cwd" | "--limit") {
            if index + 1 >= raw_args.len() {
                return Err(AppError::invalid_argument(format!(
                    "missing value for trailing {value}"
                )));
            }
            index += 2;
            continue;
        }
        if value.starts_with("--cwd=") || value.starts_with("--limit=") {
            index += 1;
            continue;
        }
        if value.starts_with('-') {
            index += 1;
            continue;
        }
        tokens.push(value.to_owned());
        if tokens.len() == 2 {
            break;
        }
        index += 1;
    }
    Ok(tokens
        .first()
        .cloned()
        .map(|domain| (domain, tokens.get(1).cloned())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn routes_service_with_globals_in_any_supported_position() {
        let route = route(&args(&[
            "ah",
            "mcp",
            "--json",
            "service",
            "install",
            "--no-start",
            "--port",
            "9000",
            "--limit=4",
        ]))
        .unwrap();
        let EarlyRoute::Service(ServiceCommand::Install(options)) = route else {
            panic!("expected install route")
        };
        assert!(options.no_start);
        assert_eq!(options.port, 9000);
        assert_eq!(options.options.limit, Some(4));
        assert_eq!(options.options.output, OutputMode::Json);
    }

    #[test]
    fn routes_only_managed_serve_early() {
        assert!(matches!(
            route(&args(&["ah", "mcp", "serve"])).unwrap(),
            EarlyRoute::NotManaged
        ));
        assert!(matches!(
            route(&args(&[
                "ah",
                "mcp",
                "serve",
                "--transport",
                "http",
                "--managed-config",
                "definition.json"
            ]))
            .unwrap(),
            EarlyRoute::ManagedServe { .. }
        ));
    }

    #[test]
    fn unrelated_domain_never_enters_managed_parser() {
        assert!(matches!(
            route(&args(&["ah", "file", "read", "mcp", "service"])).unwrap(),
            EarlyRoute::NotManaged
        ));
    }

    #[test]
    fn routes_every_lifecycle_mutation_early() {
        for (name, expected) in [
            ("start", "start"),
            ("stop", "stop"),
            ("restart", "restart"),
            ("status", "status"),
            ("uninstall", "uninstall"),
        ] {
            let route = route(&args(&["ah", "mcp", "service", name])).unwrap();
            let actual = match route {
                EarlyRoute::Service(ServiceCommand::Start { .. }) => "start",
                EarlyRoute::Service(ServiceCommand::Stop { .. }) => "stop",
                EarlyRoute::Service(ServiceCommand::Restart { .. }) => "restart",
                EarlyRoute::Service(ServiceCommand::Status { .. }) => "status",
                EarlyRoute::Service(ServiceCommand::Uninstall { .. }) => "uninstall",
                _ => "other",
            };
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn lifecycle_mutations_accept_existing_global_options_only() {
        for name in ["stop", "restart", "uninstall"] {
            let parsed = route(&args(&[
                "ah", "--cwd", ".", "mcp", "service", name, "--json", "--quiet", "--limit", "7",
            ]))
            .unwrap();
            let options = match parsed {
                EarlyRoute::Service(ServiceCommand::Stop { options })
                | EarlyRoute::Service(ServiceCommand::Restart { options })
                | EarlyRoute::Service(ServiceCommand::Uninstall { options }) => options,
                _ => panic!("expected lifecycle mutation route"),
            };
            assert_eq!(options.output, OutputMode::Json);
            assert!(options.quiet);
            assert_eq!(options.limit, Some(7));

            let error = route(&args(&["ah", "mcp", "service", name, "--force"])).unwrap_err();
            assert_eq!(error.code(), "INVALID_ARGUMENT");
        }
    }

    #[test]
    fn service_help_lists_the_complete_public_lifecycle() {
        let help = build_service_help_command().render_long_help().to_string();
        for command in ["install", "start", "stop", "restart", "status", "uninstall"] {
            assert!(
                help.contains(command),
                "missing {command} from service help"
            );
        }
        assert_eq!(
            route(&args(&["ah", "mcp", "service"])).unwrap_err().code(),
            "INVALID_ARGUMENT"
        );
    }
}
