//! What `ai install`, `ai uninstall` and `ai status` report, in the shape the
//! text and JSON renderers consume.

use super::*;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct McpReport {
    pub(crate) action: Action,
    pub(crate) registrar: &'static str,
    pub(crate) scope: Scope,
    pub(crate) transport: Option<Transport>,
    pub(crate) url: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct RulesReport {
    pub(crate) action: Action,
    pub(crate) path: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ManagedReport {
    pub(crate) action: ManagedAction,
    pub(crate) endpoint: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetReport {
    pub(crate) command: &'static str,
    pub(crate) schema_version: u32,
    pub(crate) target: &'static str,
    pub(crate) scope: Scope,
    pub(crate) changed: bool,
    pub(crate) dry_run: bool,
    pub(crate) mcp: McpReport,
    pub(crate) rules: RulesReport,
    pub(crate) managed_service: Option<ManagedReport>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct StatusReport {
    pub(crate) command: &'static str,
    pub(crate) schema_version: u32,
    pub(crate) targets: Vec<TargetStatus>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TargetStatus {
    pub(crate) target: &'static str,
    pub(crate) scope: Scope,
    pub(crate) cli: Option<&'static str>,
    pub(crate) cli_available: bool,
    /// A registration left by an older AIHelper under its previous server name.
    pub(crate) legacy_server: Option<&'static str>,
    pub(crate) mcp: McpReport,
    pub(crate) rules: RulesReport,
    pub(crate) scopes: Vec<ScopeStatus>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ScopeStatus {
    pub(crate) scope: Scope,
    pub(crate) mcp: Option<ScopedMcpReport>,
    pub(crate) rules: Option<ScopedRulesReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScopedMcpReport {
    pub(crate) action: StatusAction,
    pub(crate) registrar: &'static str,
    pub(crate) scope: Scope,
    pub(crate) transport: Option<Transport>,
    pub(crate) url: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScopedRulesReport {
    pub(crate) action: StatusAction,
    pub(crate) path: Option<String>,
    pub(crate) detail: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusAction {
    Installed,
    NotPresent,
    Unknown,
}

pub(crate) enum StatusProgress {
    Cli {
        cli: Option<&'static str>,
        available: bool,
    },
    Mcp {
        scope: Scope,
        report: ScopedMcpReport,
    },
    Rules {
        scope: Scope,
        report: ScopedRulesReport,
    },
}

pub(crate) const SCHEMA_VERSION: u32 = 1;
