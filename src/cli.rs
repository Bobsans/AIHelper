use std::path::PathBuf;

use ah_plugin_api::GlobalOptionsWire;
use clap::{Args, Parser, Subcommand};

use crate::{error::AppError, output::OutputMode};

#[derive(Debug, Parser)]
#[command(
    name = "ah",
    version,
    about = "AIHelper CLI toolbox for AI agents and developers"
)]
pub struct Cli {
    #[arg(long, global = true, help = "Return machine-readable JSON output")]
    pub json: bool,
    #[arg(long, global = true, help = "Suppress command output")]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Set working directory"
    )]
    pub cwd: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        value_name = "N",
        help = "Cap output lines/items when supported"
    )]
    pub limit: Option<usize>,
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// AI-agent focused command manual
    Ai(AiArgs),
    /// File utilities
    File(DomainInvokeArgs),
    /// Search utilities
    Search(DomainInvokeArgs),
    /// Context-reduction utilities
    Ctx(DomainInvokeArgs),
    /// Git-focused utilities
    Git(DomainInvokeArgs),
    /// Task recipe utilities
    Task(DomainInvokeArgs),
    /// Plugin management commands
    Plugins(PluginsArgs),
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Debug, Args)]
#[command(disable_help_flag = true, disable_help_subcommand = true)]
pub struct DomainInvokeArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PluginsArgs {
    #[command(subcommand)]
    pub command: PluginsCommand,
}

#[derive(Debug, Args)]
pub struct AiArgs {
    #[command(subcommand)]
    pub command: AiCommand,
}

#[derive(Debug, Subcommand)]
pub enum AiCommand {
    #[command(about = "Show full AI-agent manual for available commands")]
    Info(AiInfoArgs),
}

#[derive(Debug, Args)]
pub struct AiInfoArgs {
    #[arg(
        long,
        value_name = "DOMAIN",
        help = "Show manual only for a single command domain"
    )]
    pub domain: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum PluginsCommand {
    #[command(about = "List registered plugins")]
    List,
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

pub enum RuntimeCommand {
    PluginsList {
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

impl Cli {
    pub fn into_runtime_command(self) -> Result<RuntimeCommand, AppError> {
        let mut deferred_cwd = self.cwd;
        let mut options = GlobalOptions {
            output: if self.json {
                OutputMode::Json
            } else {
                OutputMode::Text
            },
            quiet: self.quiet,
            limit: self.limit,
        };

        if options.limit == Some(0) {
            return Err(AppError::invalid_argument("--limit must be >= 1"));
        }

        if let Some(cwd) = deferred_cwd.take() {
            std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
        }

        match self.command {
            Some(CliCommand::Ai(args)) => match args.command {
                AiCommand::Info(info_args) => Ok(RuntimeCommand::AiInfo {
                    domain: info_args.domain,
                    options,
                }),
            },
            Some(CliCommand::File(args)) => {
                let argv =
                    strip_trailing_global_flags(&args.argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: "file".to_owned(),
                    argv,
                    options,
                })
            }
            Some(CliCommand::Search(args)) => {
                let argv =
                    strip_trailing_global_flags(&args.argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: "search".to_owned(),
                    argv,
                    options,
                })
            }
            Some(CliCommand::Ctx(args)) => {
                let argv =
                    strip_trailing_global_flags(&args.argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: "ctx".to_owned(),
                    argv,
                    options,
                })
            }
            Some(CliCommand::Git(args)) => {
                let argv =
                    strip_trailing_global_flags(&args.argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: "git".to_owned(),
                    argv,
                    options,
                })
            }
            Some(CliCommand::Task(args)) => {
                let argv =
                    strip_trailing_global_flags(&args.argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: "task".to_owned(),
                    argv,
                    options,
                })
            }
            Some(CliCommand::Plugins(_)) => Ok(RuntimeCommand::PluginsList { options }),
            Some(CliCommand::External(args)) => {
                let Some((domain, argv)) = args.split_first() else {
                    return Err(AppError::invalid_argument("missing command domain"));
                };
                let argv = strip_trailing_global_flags(argv, &mut options, &mut deferred_cwd)?;
                if let Some(cwd) = deferred_cwd {
                    std::env::set_current_dir(&cwd).map_err(|source| AppError::cwd(cwd, source))?;
                }
                Ok(RuntimeCommand::Invoke {
                    domain: domain.to_owned(),
                    argv,
                    options,
                })
            }
            None => Err(AppError::invalid_argument("missing command domain")),
        }
    }
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
                filtered.push(argv[index].clone());
                index += 1;
            }
        }
    }
    Ok(filtered)
}
