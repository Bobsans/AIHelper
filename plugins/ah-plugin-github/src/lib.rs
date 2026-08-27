//! The `github` plugin: entry points, and the layout of everything else.
//!
//! What this file holds is the ABI declaration, `parse_args` and the dispatch
//! `match`. The rest is one module per kind of thing:
//!
//! | module | what it is |
//! |---|---|
//! | `args` | the clap surface: one `Args` struct per command |
//! | `wire` | GitHub's JSON shapes, and the payloads `ah` publishes |
//! | `context` | which repository, which host, which token |
//! | `api` | building and sending the requests; reading a log archive |
//! | `commands` | one function per command |
//! | `output` | the text a command prints when JSON was not asked for |
//! | `manual` | the prose and examples `--help` shows |
//! | `typed` | the catalog and the typed-call path |
//!
//! The `use` block below is deliberately the crate's shared prelude: every
//! module opens with `use super::*`, which is the pattern `typed` already used
//! when all of this lived in one file.

use std::{
    fs,
    io::{BufReader, Cursor, Read},
    path::PathBuf,
    time::Duration,
};

#[cfg(test)]
use ah_plugin_api::InvocationRequest;
use ah_plugin_sdk::{credentials, git, http, logs, poll, render, text};

use ah_plugin_api::{
    GlobalOptionsWire, InvocationResponse, ManualCommand, ManualExample, PluginManual,
    TextFormatter, TextStyle,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::{Method, blocking::Client};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
#[cfg(test)]
use std::thread;
use zip::ZipArchive;

const DOMAIN: &str = "github";
const PLUGIN_NAME: &str = "external-github";
const DESCRIPTION: &str = "GitHub Releases and Actions plugin (dynamic)";
const DEFAULT_API_URL: &str = "https://api.github.com";
const DEFAULT_API_AUTHORITY: &str = "api.github.com";
const DEFAULT_REMOTE: &str = "origin";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 15;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_MAX_LOG_BODY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_EXPANDED_LOG_BYTES: usize = 32 * 1024 * 1024;
const GIT_CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(5);

static PLUGIN_NAME_C: &[u8] = b"external-github\0";
static DOMAIN_C: &[u8] = b"github\0";
static DESCRIPTION_C: &[u8] = b"GitHub Releases and Actions plugin (dynamic)\0";

mod api;
mod args;
mod commands;
mod context;
mod manual;
mod output;
#[cfg(test)]
mod snapshots;
#[cfg(test)]
mod tests;
mod typed;
mod wire;

pub(crate) use api::*;
pub(crate) use args::*;
pub(crate) use commands::*;
pub(crate) use context::*;
pub(crate) use manual::plugin_manual;
pub(crate) use output::*;
pub(crate) use wire::*;

ah_plugin_api::define_plugin_entrypoint_v1!(
    plugin_name_c: PLUGIN_NAME_C,
    domain_c: DOMAIN_C,
    description_c: DESCRIPTION_C,
    domain: DOMAIN,
    parse_fn: parse_args,
    execute_fn: execute,
    manual_fn: plugin_manual,
    typed_catalog_fn: typed::command_catalog,
    typed_execute_fn: typed::invoke,
    typed_cancel_fn: ah_plugin_api::cancellation::cancel,
);

impl ah_plugin_api::BindResolvedSecrets for GithubCli {
    fn bind_resolved_secrets(
        &mut self,
        secrets: &std::collections::BTreeMap<String, ah_plugin_api::ResolvedSecret>,
    ) -> Result<(), InvocationResponse> {
        if let Some(token) = typed::token_from_resolved_secrets(secrets)
            .map_err(|error| error.with_error_domain(DOMAIN))?
        {
            if self.connection.token.is_some() {
                return Err(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "GitHub token credential conflicts with an inline --token",
                )
                .with_error_domain(DOMAIN));
            }
            self.connection.token = Some(token);
        }
        Ok(())
    }
}

fn parse_args(argv: &[String]) -> Result<GithubCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match GithubCli::try_parse_from(args) {
        Ok(value) => Ok(value),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                Err(InvocationResponse::ok(Some(error.to_string())))
            } else {
                Err(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    error.to_string(),
                ))
            }
        }
    }
}

fn execute(cli: GithubCli, globals: &GlobalOptionsWire) -> InvocationResponse {
    let context = match github_context(&cli.connection) {
        Ok(value) => value,
        Err(error) => return error,
    };

    match cli.command {
        GithubCommand::Repo => execute_repo(&context, globals),
        GithubCommand::Issues(args) => execute_issues(args, &context, globals),
        GithubCommand::Issue(args) => execute_issue(args, &context, globals),
        GithubCommand::Release(args) => execute_release(args, &context, globals),
        GithubCommand::Workflows => execute_workflows(&context, globals),
        GithubCommand::Workflow(args) => execute_workflow(args, &context, globals),
        GithubCommand::Runs(args) => execute_runs(args, &context, globals),
        GithubCommand::Run(args) => execute_run(args, &context, globals),
    }
}
