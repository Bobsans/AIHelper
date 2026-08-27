//! The `postgres` plugin: entry points, and the layout of everything else.
//!
//! What this file holds is the ABI declaration, `parse_args` and the dispatch
//! `match`. The rest is one module per kind of thing. Two of them exist because
//! this plugin does something the forge plugins do not - it manages its own
//! copy of `psql`:
//!
//! | module | what it is |
//! |---|---|
//! | `args` | the clap surface, plus `SecretValue` and the connection block |
//! | `wire` | one row struct per query, and the payloads `ah` publishes |
//! | `tool` | finding a usable `psql`, and why a candidate was rejected |
//! | `download` | fetching, hashing and unpacking the managed `psql` |
//! | `paths` | where the tool config and the cache live |
//! | `commands` | one function per command |
//! | `sql` | building SQL, and refusing to build the wrong SQL |
//! | `psql` | running it and reading what it wrote |
//! | `output` | the text a command prints when JSON was not asked for |
//! | `manual` | the prose and examples `--help` shows |
//! | `typed` | the catalog and the typed-call path |
//!
//! The `use` block below is deliberately the crate's shared prelude: every
//! module opens with `use super::*`, which is the pattern `typed` already used
//! when all of this lived in one file.

use std::{
    env, fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use ah_plugin_sdk::render;

use ah_plugin_api::{
    GlobalOptionsWire, InvocationResponse, ManualCommand, ManualExample, PluginManual,
    TextFormatter, TextStyle, noninteractive_command,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::blocking::Client;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const DOMAIN: &str = "postgres";
const PLUGIN_NAME: &str = "external-postgres";
const DESCRIPTION: &str = "PostgreSQL database workflow plugin (dynamic)";
const SETTINGS_VERSION: u32 = 1;
const DEFAULT_POSTGRES_VERSION: &str = "18.4";
const MIN_POSTGRES_MAJOR: u32 = 14;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_DOWNLOAD_TIMEOUT_SECS: u64 = 1800;
const AH_POSTGRES_TOOL_PATH: &str = "AH_POSTGRES_TOOL_PATH";
const POSTGRES_18_4_WINDOWS_X64_URL: &str =
    "https://get.enterprisedb.com/postgresql/postgresql-18.4-1-windows-x64-binaries.zip";
const POSTGRES_18_4_WINDOWS_X64_SHA256: &str =
    "7effe34c0bf89027b3f171447d351cbc460f4566c8d0f643daec67f140787858";

static PLUGIN_NAME_C: &[u8] = b"external-postgres\0";
static DOMAIN_C: &[u8] = b"postgres\0";
static DESCRIPTION_C: &[u8] = b"PostgreSQL database workflow plugin (dynamic)\0";

mod args;
mod commands;
mod download;
mod manual;
mod output;
mod paths;
mod psql;
#[cfg(test)]
mod snapshots;
mod sql;
mod tool;
mod typed;
mod wire;

pub(crate) use args::*;
pub(crate) use commands::*;
pub(crate) use download::*;
pub(crate) use manual::plugin_manual;
pub(crate) use output::*;
pub(crate) use paths::*;
pub(crate) use psql::*;
pub(crate) use sql::*;
pub(crate) use tool::*;
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
    typed_cancel_fn: typed::cancel,
);

impl ah_plugin_api::BindResolvedSecrets for PostgresCli {
    fn bind_resolved_secrets(
        &mut self,
        secrets: &std::collections::BTreeMap<String, ah_plugin_api::ResolvedSecret>,
    ) -> Result<(), InvocationResponse> {
        if matches!(self.command, PostgresCommand::Tool(_)) && !secrets.is_empty() {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "PostgreSQL tool commands do not accept database credentials",
            )
            .with_error_domain(DOMAIN));
        }
        self.connection.resolved_password = typed::password_from_resolved_secrets(secrets)
            .map_err(|error| error.with_error_domain(DOMAIN))?;
        Ok(())
    }
}

fn parse_args(argv: &[String]) -> Result<PostgresCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match PostgresCli::try_parse_from(args) {
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

fn execute(cli: PostgresCli, globals: &GlobalOptionsWire) -> InvocationResponse {
    if let PostgresCommand::Tool(args) = cli.command {
        return execute_tool(args, &cli.tool, globals);
    }

    let context = match resolve_operational_tool(&cli.tool) {
        Ok(value) => value,
        Err(response) => return response,
    };

    match cli.command {
        PostgresCommand::Tool(_) => unreachable!("tool command handled before resolver"),
        PostgresCommand::Ping => execute_ping(&context, &cli.connection, globals),
        PostgresCommand::Info => execute_info(&context, &cli.connection, globals),
        PostgresCommand::Databases => execute_databases(&context, &cli.connection, globals),
        PostgresCommand::Schemas(args) => execute_schemas(args, &context, &cli.connection, globals),
        PostgresCommand::Tables(args) => execute_relations(
            "postgres.tables",
            args,
            "tables",
            &context,
            &cli.connection,
            globals,
        ),
        PostgresCommand::Views(args) => execute_relations(
            "postgres.views",
            args,
            "views",
            &context,
            &cli.connection,
            globals,
        ),
        PostgresCommand::Describe(args) => {
            execute_describe(args, &context, &cli.connection, globals)
        }
        PostgresCommand::Indexes(args) => execute_indexes(args, &context, &cli.connection, globals),
        PostgresCommand::Extensions(args) => {
            execute_extensions(args, &context, &cli.connection, globals)
        }
        PostgresCommand::Query(args) => execute_query(args, &context, &cli.connection, globals),
        PostgresCommand::Exec(args) => execute_exec(args, &context, &cli.connection, globals),
        PostgresCommand::Explain(args) => execute_explain(args, &context, &cli.connection, globals),
        PostgresCommand::Activity(args) => {
            execute_activity(args, &context, &cli.connection, globals)
        }
        PostgresCommand::Locks(args) => execute_locks(args, &context, &cli.connection, globals),
        PostgresCommand::Size(args) => execute_size(args, &context, &cli.connection, globals),
        PostgresCommand::Settings(args) => {
            execute_settings(args, &context, &cli.connection, globals)
        }
    }
}
