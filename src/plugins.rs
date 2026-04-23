use std::sync::Arc;

use ah_plugin_api::{InvocationRequest, InvocationResponse, PluginMetadata};
use ah_runtime::BuiltinPlugin;
use clap::{CommandFactory, Parser, error::ErrorKind};

use crate::{cli::GlobalOptions, commands, error::AppError};

#[derive(Debug, Parser)]
struct FilePluginCli {
    #[command(flatten)]
    args: commands::file::FileArgs,
}

#[derive(Debug, Parser)]
struct SearchPluginCli {
    #[command(flatten)]
    args: commands::search::SearchArgs,
}

#[derive(Debug, Parser)]
struct CtxPluginCli {
    #[command(flatten)]
    args: commands::ctx::CtxArgs,
}

#[derive(Debug, Parser)]
struct GitPluginCli {
    #[command(flatten)]
    args: commands::git::GitArgs,
}

#[derive(Debug, Parser)]
struct TaskPluginCli {
    #[command(flatten)]
    args: commands::task::TaskArgs,
}

pub fn builtins() -> Vec<Arc<dyn BuiltinPlugin>> {
    vec![
        Arc::new(FileBuiltinPlugin),
        Arc::new(SearchBuiltinPlugin),
        Arc::new(CtxBuiltinPlugin),
        Arc::new(GitBuiltinPlugin),
        Arc::new(TaskBuiltinPlugin),
    ]
}

struct FileBuiltinPlugin;
struct SearchBuiltinPlugin;
struct CtxBuiltinPlugin;
struct GitBuiltinPlugin;
struct TaskBuiltinPlugin;

impl BuiltinPlugin for FileBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-file".to_owned(),
            domain: "file".to_owned(),
            description: "File operations plugin (built-in)".to_owned(),
            abi_version: 1,
        }
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<FilePluginCli>("file", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::file::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for SearchBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-search".to_owned(),
            domain: "search".to_owned(),
            description: "Search operations plugin (built-in)".to_owned(),
            abi_version: 1,
        }
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<SearchPluginCli>("search", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::search::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for CtxBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-ctx".to_owned(),
            domain: "ctx".to_owned(),
            description: "Context utilities plugin (built-in)".to_owned(),
            abi_version: 1,
        }
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<CtxPluginCli>("ctx", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::ctx::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for GitBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-git".to_owned(),
            domain: "git".to_owned(),
            description: "Git utilities plugin (built-in)".to_owned(),
            abi_version: 1,
        }
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<GitPluginCli>("git", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::git::execute(parsed.args, &options))
    }
}

impl BuiltinPlugin for TaskBuiltinPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            plugin_name: "builtin-task".to_owned(),
            domain: "task".to_owned(),
            description: "Task recipe plugin (built-in)".to_owned(),
            abi_version: 1,
        }
    }

    fn invoke(&self, request: &InvocationRequest) -> InvocationResponse {
        let parsed = match parse_args::<TaskPluginCli>("task", &request.argv) {
            ParseOutcome::Parsed(value) => value,
            ParseOutcome::Response(response) => return response,
        };
        let options = GlobalOptions::from(request.globals.clone());
        map_execute(commands::task::execute(parsed.args, &options))
    }
}

enum ParseOutcome<T> {
    Parsed(T),
    Response(InvocationResponse),
}

fn parse_args<T: Parser + CommandFactory>(domain: &str, argv: &[String]) -> ParseOutcome<T> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(domain.to_owned());
    args.extend(argv.iter().cloned());

    match T::try_parse_from(args) {
        Ok(value) => ParseOutcome::Parsed(value),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                ParseOutcome::Response(InvocationResponse::ok(Some(error.to_string())))
            } else {
                ParseOutcome::Response(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    error.to_string(),
                ))
            }
        }
    }
}

fn map_execute(result: Result<(), AppError>) -> InvocationResponse {
    match result {
        Ok(()) => InvocationResponse::ok(None),
        Err(error) => InvocationResponse::error(error.code(), error.to_string()),
    }
}
