//! The `gitlab` plugin: entry points, and the layout of everything else.
//!
//! What this file holds is the ABI declaration, `parse_args` and the dispatch
//! `match`. The rest is one module per kind of thing, the same shape the
//! `github` plugin uses:
//!
//! | module | what it is |
//! |---|---|
//! | `args` | the clap surface: one `Args` struct per command |
//! | `wire` | GitLab's JSON shapes, and the payloads `ah` publishes |
//! | `context` | which project, which host, which token |
//! | `api` | building and sending the requests; reading a job trace |
//! | `commands` | one function per command |
//! | `output` | the text a command prints when JSON was not asked for |
//! | `manual` | the prose and examples `--help` shows |
//! | `typed` | the catalog and the typed-call path |
//!
//! The `use` block below is deliberately the crate's shared prelude: every
//! module opens with `use super::*`, which is the pattern `typed` already used
//! when all of this lived in one file.

use std::{fs, io::BufReader, path::PathBuf, time::Duration};

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

const DOMAIN: &str = "gitlab";
const PLUGIN_NAME: &str = "external-gitlab";
const DESCRIPTION: &str = "GitLab Releases and Pipelines plugin (dynamic)";
const DEFAULT_HOST: &str = "https://gitlab.com";
const DEFAULT_HOST_AUTHORITY: &str = "gitlab.com";
const DEFAULT_REMOTE: &str = "origin";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 15;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_MAX_TRACE_BODY_BYTES: usize = 8 * 1024 * 1024;
const GIT_CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(5);
const ISSUE_DESIGNS_QUERY: &str = r#"
query IssueDesigns($fullPath: ID!, $iid: String!, $first: Int!) {
  project(fullPath: $fullPath) {
    issue(iid: $iid) {
      designCollection {
        designs(first: $first) {
          nodes {
            id
            filename
            fullPath
            image
            imageV432x230
            notesCount
            event
          }
        }
      }
    }
  }
}
"#;

static PLUGIN_NAME_C: &[u8] = b"external-gitlab\0";
static DOMAIN_C: &[u8] = b"gitlab\0";
static DESCRIPTION_C: &[u8] = b"GitLab Releases and Pipelines plugin (dynamic)\0";

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

impl ah_plugin_api::BindResolvedSecrets for GitlabCli {
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
                    "GitLab token credential conflicts with an inline --token",
                )
                .with_error_domain(DOMAIN));
            }
            self.connection.token = Some(token);
        }
        Ok(())
    }
}

fn parse_args(argv: &[String]) -> Result<GitlabCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match GitlabCli::try_parse_from(args) {
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

fn execute(cli: GitlabCli, globals: &GlobalOptionsWire) -> InvocationResponse {
    let context = match gitlab_context(&cli.connection) {
        Ok(value) => value,
        Err(error) => return error,
    };

    match cli.command {
        GitlabCommand::Project => execute_project(&context, globals),
        GitlabCommand::Issues(args) => execute_issues(args, &context, globals),
        GitlabCommand::Issue(args) => execute_issue(args, &context, globals),
        GitlabCommand::Releases => execute_releases(&context, globals),
        GitlabCommand::Release(args) => execute_release(args, &context, globals),
        GitlabCommand::Pipelines(args) => execute_pipelines(args, &context, globals),
        GitlabCommand::Pipeline(args) => execute_pipeline(args, &context, globals),
        GitlabCommand::Job(args) => execute_job(args, &context, globals),
    }
}
