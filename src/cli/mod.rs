//! The command line: what it declares, how a parse becomes a command, and the
//! arguments that have to be read before clap can run.
//!
//! The 1177 production lines this came from held the clap tree, the parse, the
//! `run check` passthrough walker, argv redaction and the did-you-mean
//! suggester in one file:
//!
//! | Module        | Owns                                                   |
//! |---------------|--------------------------------------------------------|
//! | `command`     | the clap tree, declaration only                        |
//! | `parse`       | one `ArgMatches` into one `RuntimeCommand`             |
//! | `passthrough` | argv this process reads before clap does               |
//! | `redact`      | hiding a secret's value from the event log             |
//! | `suggest`     | the nearest command name when there is no match        |

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

mod command;
mod parse;
mod passthrough;
mod redact;
mod suggest;
pub(crate) use command::build_cli_command;
use command::{
    HOST_COMMAND_AI, HOST_COMMAND_MCP, HOST_COMMAND_PLUGINS, HOST_COMMAND_SECRETS,
    HOST_COMMAND_UPGRADE, top_level_domain_summary,
};
pub use parse::parse_runtime_command;
use passthrough::{host_option_end, prepare_run_check_passthrough};
pub use passthrough::{initial_cwd_from_raw_args, resolve_request_dir};
pub(crate) use redact::redact_secret_command_argv;
use suggest::decorate_cli_parse_error;
pub(crate) use suggest::suggest_top_level_command;
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use command::build_ai_command;
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use parse::has_decision_flags;
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use passthrough::extract_last_cwd;

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
        request: crate::updater::request::UpgradeRequest,
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

#[cfg(test)]
mod tests;
