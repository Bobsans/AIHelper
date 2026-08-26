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

use ah_plugin_api::{PluginMetadata, normalize_invocation_argv};
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
        request: ah_updater::request::UpgradeRequest,
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

/// How a request reports, wherever it came from.
///
/// Defined in `ah-output` beside the emitter that consumes it, and re-exported
/// here because the CLI is what fills it in. Every command module reads it, so
/// it cannot live in the clap layer.
pub use ah_output::GlobalOptions;

#[cfg(test)]
mod tests;
