//! What one `mcp service` invocation asks the mechanism to do.
//!
//! No `GlobalOptions`: the mechanism does not render, so the only CLI value it
//! needs is `--limit`, which is written into the service definition and
//! therefore outlives the invocation that set it.

use super::{
    model::{MutationOutput, UninstallOutput},
    output::StatusOutput,
};

#[derive(Debug)]
pub enum Operation {
    Install(InstallSettings),
    Start,
    Stop,
    Restart,
    Status,
    Uninstall,
}

#[derive(Debug, Clone)]
pub struct InstallSettings {
    pub no_start: bool,
    pub port: u16,
    pub max_active: usize,
    pub default_timeout_ms: u64,
    /// Written into the definition, so the served output is capped the same way
    /// whoever installed the service asked for.
    pub limit: Option<usize>,
}

/// What an operation produced, for the CLI to render.
///
/// Boxed because a status report carries every observed layer and is an order
/// of magnitude larger than a mutation's; the enum is returned once per
/// invocation, so the indirection costs nothing that matters.
#[derive(Debug)]
pub enum Report {
    Mutation(Box<MutationOutput>),
    Status(Box<StatusOutput>),
    Uninstall(Box<UninstallOutput>),
}
