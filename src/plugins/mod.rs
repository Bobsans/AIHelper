//! The eight builtin plugins, one module each.
//!
//! They used to share a 921-line file in which every domain's CLI struct, its
//! metadata, its manual and its `BuiltinPlugin` impl were four separate runs of
//! eight near-identical blocks. Adding a builtin meant finding four places;
//! now it is one file, and the four things a builtin needs sit next to each
//! other in it.
//!
//! What stays here is what all eight share: the argv parse, the execute
//! mapping, and the list itself.

use std::sync::Arc;

use ah_plugin_api::{
    CommandCatalog, GlobalOptionsWire, InvocationRequest, InvocationResponse, ManualCommand,
    ManualExample, PluginCompatibility, PluginManual, PluginMetadata, RequiredTool,
    TypedInvocationRequest, TypedInvocationResponse, normalize_invocation_argv,
    plugin_capabilities,
};
use ah_runtime::{BuiltinPlugin, OutputSink};
use clap::{CommandFactory, Parser, error::ErrorKind};

use crate::{cli::GlobalOptions, commands, error::AppError};

mod ctx;
mod file;
mod git;
mod http;
mod project;
mod run;
mod search;
mod task;
use ctx::CtxBuiltinPlugin;
use file::FileBuiltinPlugin;
use git::{GitBuiltinPlugin, git_required_tool};
use http::HttpBuiltinPlugin;
use project::ProjectBuiltinPlugin;
use run::RunBuiltinPlugin;
use search::SearchBuiltinPlugin;
use task::TaskBuiltinPlugin;
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use ctx::{CtxPluginCli, ctx_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use file::{FilePluginCli, file_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use git::{GitPluginCli, git_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use http::{HttpPluginCli, http_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use project::{ProjectPluginCli, project_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use run::{RunPluginCli, run_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use search::{SearchPluginCli, search_manual};
// Reached only from `tests`, which globs this module, not its children.
#[cfg(test)]
use task::{TaskPluginCli, task_manual};

pub fn builtins() -> Vec<Arc<dyn BuiltinPlugin>> {
    vec![
        Arc::new(FileBuiltinPlugin),
        Arc::new(SearchBuiltinPlugin),
        Arc::new(CtxBuiltinPlugin),
        Arc::new(GitBuiltinPlugin),
        Arc::new(ProjectBuiltinPlugin),
        Arc::new(RunBuiltinPlugin),
        Arc::new(HttpBuiltinPlugin),
        Arc::new(TaskBuiltinPlugin),
    ]
}

enum ParseOutcome<T> {
    Parsed(T, GlobalOptions),
    Response(InvocationResponse),
}

fn request_command(request: &InvocationRequest) -> Option<String> {
    normalize_invocation_argv(&request.argv, request.globals.clone())
        .ok()
        .and_then(|normalized| normalized.argv.into_iter().next())
}

fn parse_args<T: Parser + CommandFactory>(
    domain: &str,
    argv: &[String],
    globals: GlobalOptionsWire,
) -> ParseOutcome<T> {
    let normalized = match normalize_invocation_argv(argv, globals) {
        Ok(value) => value,
        Err(error) => return ParseOutcome::Response(error.with_error_domain(domain)),
    };

    let options = GlobalOptions::from(normalized.globals);
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(domain.to_owned());
    args.extend(normalized.argv);

    match T::try_parse_from(args) {
        Ok(value) => ParseOutcome::Parsed(value, options),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                ParseOutcome::Response(InvocationResponse::ok(Some(error.to_string())))
            } else {
                ParseOutcome::Response(
                    InvocationResponse::error("INVALID_ARGUMENT", error.to_string())
                        .with_error_domain(domain),
                )
            }
        }
    }
}

fn map_execute(domain: &str, result: Result<(), AppError>) -> InvocationResponse {
    match result {
        Ok(()) => InvocationResponse::ok(None),
        Err(error) => InvocationResponse::error_diagnostic(error.diagnostic().with_domain(domain)),
    }
}

#[cfg(test)]
mod tests;
