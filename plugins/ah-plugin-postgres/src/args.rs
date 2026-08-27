//! The command-line surface: one `Args` struct per command, plus the
//! defaults `serde` and `schemars` need to see as functions.
//!
//! Two things here are not pure declaration and belong with the arguments
//! anyway: `ConnectionArgs` knows how to turn itself into the environment
//! `psql` reads, and `SecretValue` is a `String` whose `Debug` prints nothing -
//! a password reaches this struct, so its own formatter is the last line of
//! defence against a log statement.

use super::*;

#[derive(Debug, Parser)]
#[command(name = "postgres", about = "PostgreSQL database workflow helpers")]
pub(crate) struct PostgresCli {
    #[command(flatten)]
    pub(crate) tool: ToolResolverArgs,
    #[command(flatten)]
    pub(crate) connection: ConnectionArgs,
    #[command(subcommand)]
    pub(crate) command: PostgresCommand,
}

// Only the path half of `ToolResolverArgs`. A doc comment here would be
// published as the schema `description`.
//
// A toolchain command must not accept `ensure_tool`: `postgres.tool.status` is
// declared read-only and low risk, and honouring it there would let a caller
// trigger a download from a command that promises not to write.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolPathArgs {
    #[schemars(description = "Explicit psql executable or tool directory resolved against cwd.")]
    pub(crate) tool_path: Option<PathBuf>,
}

#[derive(Debug, Args, Clone, Deserialize, JsonSchema)]
pub(crate) struct ToolResolverArgs {
    #[arg(long, global = true, value_name = "PATH")]
    #[schemars(description = "Explicit psql executable or tool directory resolved against cwd.")]
    pub(crate) tool_path: Option<PathBuf>,
    #[arg(long, global = true)]
    #[serde(default)]
    #[schemars(
        default,
        description = "Download the managed toolchain when no usable psql exists. This writes the shared cache."
    )]
    pub(crate) ensure_tool: bool,
}

#[derive(Debug, Args, Clone, Deserialize, JsonSchema)]
pub(crate) struct ConnectionArgs {
    #[arg(long, global = true, value_name = "HOST")]
    #[schemars(description = "PostgreSQL host.")]
    pub(crate) host: Option<String>,
    #[arg(long, global = true, value_name = "PORT")]
    #[schemars(range(min = 1, max = 65535))]
    pub(crate) port: Option<u16>,
    #[arg(long, global = true, value_name = "NAME")]
    #[schemars(description = "PostgreSQL database name.")]
    pub(crate) database: Option<String>,
    #[arg(long, global = true, value_name = "USER")]
    #[schemars(description = "PostgreSQL user.")]
    pub(crate) user: Option<String>,
    #[arg(long, global = true, value_name = "NAME")]
    #[schemars(description = "libpq service name.")]
    pub(crate) service: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "MODE",
        value_parser = ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"]
    )]
    #[schemars(extend("enum" = ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"]))]
    pub(crate) sslmode: Option<String>,
    #[arg(long, global = true, value_name = "ENV_VAR")]
    #[schemars(
        description = "For password-protected servers, name an environment variable that already exists in this process; its value is passed to psql as PGPASSWORD. Never pass the password itself, and never guess a variable name. Prefer the database credential slot: call secrets.list with kind=postgres, and if no secret matches, ask the user to create one instead."
    )]
    pub(crate) password_env: Option<String>,
    // Bound from the vault, never sent by the caller.
    #[arg(skip)]
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) resolved_password: Option<SecretValue>,
    #[arg(long, global = true, default_value_t = DEFAULT_CONNECT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(default = "default_connect_timeout_secs")]
    #[schemars(
        default = "default_connect_timeout_secs",
        range(min = 1),
        description = "Connection timeout capped by the MCP request deadline."
    )]
    pub(crate) connect_timeout_secs: u64,
    #[arg(long, global = true, value_name = "MILLISECONDS")]
    #[schemars(
        range(min = 1),
        description = "Server statement timeout capped by the MCP request deadline."
    )]
    pub(crate) statement_timeout_ms: Option<u64>,
}

pub(crate) fn default_connect_timeout_secs() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_SECS
}

impl ConnectionArgs {
    /// A toolchain command never connects, so it carries an unset block rather
    /// than pretending to have connection arguments the caller did not send.
    pub(crate) fn unset() -> Self {
        Self {
            host: None,
            port: None,
            database: None,
            user: None,
            service: None,
            sslmode: None,
            password_env: None,
            resolved_password: None,
            connect_timeout_secs: DEFAULT_CONNECT_TIMEOUT_SECS,
            statement_timeout_ms: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct SecretValue(String);

impl SecretValue {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum PostgresCommand {
    #[command(about = "Manage the PostgreSQL client toolchain")]
    Tool(ToolArgs),
    #[command(about = "Check that psql can connect to the selected database")]
    Ping,
    #[command(about = "Show selected PostgreSQL server and session metadata")]
    Info,
    #[command(about = "List databases")]
    Databases,
    #[command(about = "List schemas")]
    Schemas(IncludeSystemArgs),
    #[command(about = "List tables and table-like relations")]
    Tables(RelationListArgs),
    #[command(about = "List views")]
    Views(RelationListArgs),
    #[command(about = "Describe a table, view, or materialized view")]
    Describe(DescribeArgs),
    #[command(about = "List indexes")]
    Indexes(IndexesArgs),
    #[command(about = "List installed or available extensions")]
    Extensions(ExtensionsArgs),
    #[command(about = "Run a read-only SQL query")]
    Query(QueryArgs),
    #[command(about = "Execute explicit SQL mutations or admin commands")]
    Exec(ExecArgs),
    #[command(about = "Explain a SQL query plan")]
    Explain(ExplainArgs),
    #[command(about = "Show pg_stat_activity rows")]
    Activity(ActivityArgs),
    #[command(about = "Show lock and blocking diagnostics")]
    Locks(LocksArgs),
    #[command(about = "Show database, schema, or table sizes")]
    Size(SizeArgs),
    #[command(about = "Show PostgreSQL settings")]
    Settings(SettingsArgs),
}

#[derive(Debug, Args)]
pub(crate) struct ToolArgs {
    #[command(subcommand)]
    pub(crate) command: ToolCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ToolCommand {
    #[command(about = "Show resolved PostgreSQL toolchain status")]
    Status,
    #[command(about = "Download a managed PostgreSQL toolchain")]
    Download(ToolDownloadArgs),
    #[command(about = "Persist an explicit PostgreSQL toolchain path")]
    Use(ToolUseArgs),
    #[command(about = "Remove managed PostgreSQL toolchain cache")]
    Cleanup(ToolCleanupArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolDownloadArgs {
    #[arg(long, default_value = DEFAULT_POSTGRES_VERSION, value_name = "VERSION")]
    #[serde(default = "default_postgres_version")]
    #[schemars(default = "default_postgres_version", length(min = 1))]
    pub(crate) version: String,
    /// Replace an existing managed toolchain.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) force: bool,
    /// Download timeout capped by the MCP request deadline.
    #[arg(skip = DEFAULT_DOWNLOAD_TIMEOUT_SECS)]
    #[serde(rename = "timeout_secs", default = "default_download_timeout_secs")]
    #[schemars(
        rename = "timeout_secs",
        default = "default_download_timeout_secs",
        range(min = 1)
    )]
    pub(crate) download_timeout_secs: u64,
}

pub(crate) fn default_postgres_version() -> String {
    DEFAULT_POSTGRES_VERSION.to_owned()
}

pub(crate) fn default_download_timeout_secs() -> u64 {
    DEFAULT_DOWNLOAD_TIMEOUT_SECS
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolUseArgs {
    /// Tool executable or directory resolved against cwd.
    #[arg(long, value_name = "PATH")]
    #[schemars(extend("minLength" = 1))]
    pub(crate) path: PathBuf,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolCleanupArgs {
    /// Managed version to remove; omit to remove every cached version.
    #[arg(long, value_name = "VERSION")]
    pub(crate) version: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IncludeSystemArgs {
    /// Include system schemas.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) include_system: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelationListArgs {
    /// Restrict results to this schema.
    #[arg(long, value_name = "NAME")]
    pub(crate) schema: Option<String>,
    /// Include system relations.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) include_system: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DescribeArgs {
    /// Relation name as NAME or SCHEMA.NAME.
    #[schemars(length(min = 1))]
    pub(crate) object: String,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IndexesArgs {
    /// Restrict results to this schema.
    #[arg(long, value_name = "NAME")]
    pub(crate) schema: Option<String>,
    /// Restrict results to this table.
    #[arg(long, value_name = "NAME")]
    pub(crate) table: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtensionsArgs {
    /// Include available but not installed extensions.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) available: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryArgs {
    /// Inline SQL text. Exactly one of sql or file is required.
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    pub(crate) sql: Option<String>,
    /// UTF-8 SQL file resolved against cwd. Exactly one of sql or file is required.
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecArgs {
    /// Inline SQL text. Exactly one of sql or file is required.
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    pub(crate) sql: Option<String>,
    /// UTF-8 SQL file resolved against cwd. Exactly one of sql or file is required.
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,
    /// Execute all SQL in one transaction.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) single_transaction: bool,
    /// Required explicit confirmation for arbitrary SQL mutation.
    // Deliberately not defaulted: omitting it must fail validation rather than
    // silently mean "no".
    #[arg(long)]
    #[schemars(extend("const" = true))]
    pub(crate) yes: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplainArgs {
    /// Inline SQL text. Exactly one of sql or file is required.
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    pub(crate) sql: Option<String>,
    /// UTF-8 SQL file resolved against cwd. Exactly one of sql or file is required.
    #[arg(long, value_name = "PATH")]
    pub(crate) file: Option<PathBuf>,
    /// Execute the SQL while collecting actual plan statistics.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) analyze: bool,
    /// Include buffer usage in the plan.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) buffers: bool,
    /// Required when analyze=true because the SQL is executed.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) yes: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityArgs {
    /// Show only active sessions.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) active: bool,
    /// Show only sessions idle in a transaction.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) idle_in_tx: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocksArgs {
    /// Return only locks with a known blocking session.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) blocking: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SizeArgs {
    /// Show aggregate size for a schema or qualify a table.
    #[arg(long, value_name = "NAME")]
    pub(crate) schema: Option<String>,
    /// Show table size; may be NAME or SCHEMA.NAME.
    #[arg(long, value_name = "NAME")]
    pub(crate) table: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsArgs {
    /// Return only settings whose source is not default.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) changed: bool,
}
