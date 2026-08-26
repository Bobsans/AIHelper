//! What `postgres` publishes to the command catalog: one descriptor per
//! command, with its schemas, declared effects, risk and secret slot.
//!
//! Declaration, not logic - which is why it outgrew the dispatch it used to
//! share a file with.

use super::*;

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        PLUGIN_NAME,
        DOMAIN,
        vec![
            tool_status_descriptor(),
            tool_download_descriptor(),
            tool_use_descriptor(),
            tool_cleanup_descriptor(),
            ping_descriptor(),
            info_descriptor(),
            databases_descriptor(),
            schemas_descriptor(),
            relations_descriptor("postgres.tables", "List PostgreSQL tables"),
            relations_descriptor("postgres.views", "List PostgreSQL views"),
            describe_descriptor(),
            indexes_descriptor(),
            extensions_descriptor(),
            query_descriptor(),
            exec_descriptor(),
            explain_descriptor(),
            activity_descriptor(),
            locks_descriptor(),
            size_descriptor(),
            settings_descriptor(),
        ],
    )
}

pub(super) fn tool_status_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.status",
        "Inspect PostgreSQL toolchain",
        "Resolve psql candidates and report the selected client toolchain.",
        input_schema_for::<ToolPathArgs>(),
        output_schema_for::<ToolStatusOutput>("postgres.tool.status"),
        CommandEffects::new(
            true,
            false,
            true,
            false,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::ConfigurationRead,
                CommandEffect::ProcessSpawn,
            ],
            RiskLevel::Low,
            "Reads tool configuration and may execute candidate psql binaries with --version.",
            Reversibility::Yes,
        ),
    )
}

pub(super) fn tool_download_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.download",
        "Download PostgreSQL toolchain",
        "Download, verify, and unpack the supported PostgreSQL client toolchain.",
        input_schema_for::<ToolDownloadArgs>(),
        output_schema_for::<ToolDownloadOutput>("postgres.tool.download"),
        CommandEffects::new(
            false,
            false,
            false,
            true,
            vec![
                CommandEffect::NetworkRead,
                CommandEffect::FilesystemRead,
                CommandEffect::FilesystemWrite,
                CommandEffect::ProcessSpawn,
            ],
            RiskLevel::High,
            "Downloads an executable archive from the built-in vendor URL, verifies its checksum, and writes or replaces files in the shared PostgreSQL tool cache.",
            Reversibility::Yes,
        ),
    )
}

pub(super) fn tool_use_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.use",
        "Select PostgreSQL toolchain",
        "Validate a psql toolchain path and persist it as the shared default.",
        input_schema_for::<ToolUseArgs>(),
        output_schema_for::<ToolUseOutput>("postgres.tool.use"),
        CommandEffects::new(
            false,
            false,
            true,
            false,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::ConfigurationWrite,
                CommandEffect::ProcessSpawn,
            ],
            RiskLevel::High,
            "Executes the selected psql binary with --version and changes the shared PostgreSQL tool configuration used by later tasks.",
            Reversibility::Yes,
        ),
    )
}

pub(super) fn tool_cleanup_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.cleanup",
        "Clean PostgreSQL tool cache",
        "Delete one version or all versions from the managed PostgreSQL tool cache.",
        input_schema_for::<ToolCleanupArgs>(),
        output_schema_for::<ToolCleanupOutput>("postgres.tool.cleanup"),
        CommandEffects::new(
            false,
            true,
            true,
            false,
            vec![CommandEffect::FilesystemDelete],
            RiskLevel::High,
            "Permanently deletes managed PostgreSQL tool directories from the shared cache; concurrent tasks using those binaries may fail.",
            Reversibility::No,
        ),
    )
}

pub(super) fn ping_descriptor() -> CommandDescriptor {
    database_descriptor::<NoArgs>(
        "postgres.ping",
        "Ping PostgreSQL",
        "Connect and return server plus session identity metadata.",
        output_schema_for::<InfoOutput>("postgres.ping"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

pub(super) fn info_descriptor() -> CommandDescriptor {
    database_descriptor::<NoArgs>(
        "postgres.info",
        "Inspect PostgreSQL session",
        "Return selected server and session metadata.",
        output_schema_for::<InfoOutput>("postgres.info"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

pub(super) fn databases_descriptor() -> CommandDescriptor {
    rows_descriptor::<NoArgs, DatabaseRow>(
        "postgres.databases",
        "List PostgreSQL databases",
        "Reads database names, owners, encodings, connection flags, and visible size information.",
    )
}

pub(super) fn schemas_descriptor() -> CommandDescriptor {
    rows_descriptor::<IncludeSystemArgs, SchemaRow>(
        "postgres.schemas",
        "List PostgreSQL schemas",
        "Reads schema names and owners, optionally including system schemas.",
    )
}

pub(super) fn relations_descriptor(id: &str, title: &str) -> CommandDescriptor {
    rows_descriptor::<RelationListArgs, RelationRow>(
        id,
        title,
        "Reads relation names, owners, row estimates, and total sizes.",
    )
}

pub(super) fn describe_descriptor() -> CommandDescriptor {
    database_descriptor::<DescribeArgs>(
        "postgres.describe",
        "Describe PostgreSQL relation",
        "Describe a table, view, or materialized view.",
        output_schema_for::<DescribeOutput>("postgres.describe"),
        "Reads relation, column, index, and constraint definitions that may reveal database structure.",
    )
}

pub(super) fn indexes_descriptor() -> CommandDescriptor {
    rows_descriptor::<IndexesArgs, IndexRow>(
        "postgres.indexes",
        "List PostgreSQL indexes",
        "Reads index names and full definitions.",
    )
}

pub(super) fn extensions_descriptor() -> CommandDescriptor {
    rows_descriptor::<ExtensionsArgs, ExtensionRow>(
        "postgres.extensions",
        "List PostgreSQL extensions",
        "Reads installed extension metadata or the server's available extension catalog.",
    )
}

pub(super) fn query_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.query",
        "Query PostgreSQL",
        "Run SQL restricted to SELECT, WITH, TABLE, or VALUES in a read-only transaction.",
        input_schema_for::<DbWire<QueryArgs>>(),
        output_schema_for::<QueryOutput>("postgres.query"),
        CommandEffects::new(
            false,
            false,
            false,
            true,
            database_effects(),
            RiskLevel::High,
            "Sends arbitrary read-oriented SQL to the selected database and may expose sensitive rows; PostgreSQL functions can have external effects despite the read-only transaction, and ensure_tool=true may write the shared tool cache.",
            Reversibility::Unknown,
        ),
    )
    .with_secret_slot(postgres_secret_slot())
    .with_example(CommandExample::new(
        "Read the current time",
        json!({"sql": "select now() as current_time"}),
    ))
}

pub(super) fn exec_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.exec",
        "Execute PostgreSQL SQL",
        "Execute explicitly confirmed SQL mutations or administrative commands.",
        input_schema_for::<DbWire<ExecArgs>>(),
        output_schema_for::<ExecOutput>("postgres.exec"),
        CommandEffects::new(
            false,
            true,
            false,
            true,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::FilesystemWrite,
                CommandEffect::ProcessSpawn,
                CommandEffect::NetworkWrite,
                CommandEffect::ExternalWrite,
                CommandEffect::ConfigurationRead,
            ],
            RiskLevel::Critical,
            "Executes arbitrary SQL with the configured database privileges; it can modify or delete data and schema, lock objects, invoke extensions, and affect other sessions. Effects may be irreversible even with single_transaction=true.",
            Reversibility::Unknown,
        ),
    )
    .with_secret_slot(postgres_secret_slot())
}

pub(super) fn explain_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.explain",
        "Explain PostgreSQL SQL",
        "Return a query plan; analyze=true executes the supplied SQL.",
        input_schema_for::<DbWire<ExplainArgs>>(),
        output_schema_for::<ExplainOutput>("postgres.explain"),
        CommandEffects::new(
            false,
            true,
            false,
            true,
            vec![
                CommandEffect::FilesystemRead,
                CommandEffect::FilesystemWrite,
                CommandEffect::ProcessSpawn,
                CommandEffect::NetworkWrite,
                CommandEffect::ExternalWrite,
                CommandEffect::ConfigurationRead,
            ],
            RiskLevel::Critical,
            "Without analyze, PostgreSQL plans but does not execute the SQL. With analyze=true, arbitrary SQL is executed and can mutate or delete database state, acquire locks, and invoke external functions.",
            Reversibility::Unknown,
        ),
    )
    .with_secret_slot(postgres_secret_slot())
}

pub(super) fn activity_descriptor() -> CommandDescriptor {
    rows_descriptor::<ActivityArgs, ActivityRow>(
        "postgres.activity",
        "Inspect PostgreSQL activity",
        "Reads session identities, client addresses, wait states, timestamps, and truncated SQL text from pg_stat_activity.",
    )
}

pub(super) fn locks_descriptor() -> CommandDescriptor {
    rows_descriptor::<LocksArgs, LockRow>(
        "postgres.locks",
        "Inspect PostgreSQL locks",
        "Reads blocked and blocking session identities plus truncated SQL text.",
    )
}

pub(super) fn size_descriptor() -> CommandDescriptor {
    rows_descriptor::<SizeArgs, SizeRow>(
        "postgres.size",
        "Inspect PostgreSQL sizes",
        "Reads database, schema, or relation size statistics.",
    )
}

pub(super) fn settings_descriptor() -> CommandDescriptor {
    rows_descriptor::<SettingsArgs, SettingRow>(
        "postgres.settings",
        "Inspect PostgreSQL settings",
        "Reads server settings, sources, units, and descriptions; configuration values may contain sensitive operational details.",
    )
}

pub(super) fn rows_descriptor<A: JsonSchema, T: JsonSchema>(
    id: &str,
    title: &str,
    impact: &str,
) -> CommandDescriptor {
    database_descriptor::<A>(
        id,
        title,
        title,
        output_schema_for::<RowsOutput<T>>(id),
        impact,
    )
}

pub(super) fn database_descriptor<A: JsonSchema>(
    id: &str,
    title: &str,
    description: &str,
    output_schema: Value,
    impact: &str,
) -> CommandDescriptor {
    descriptor(
        id,
        title,
        description,
        input_schema_for::<DbWire<A>>(),
        output_schema,
        CommandEffects::new(
            false,
            false,
            false,
            true,
            database_effects(),
            RiskLevel::High,
            format!(
                "{impact} Runs psql as a child process. ensure_tool=true may download and write a shared client toolchain before connecting."
            ),
            Reversibility::Yes,
        ),
    )
    .with_secret_slot(postgres_secret_slot())
}

pub(super) fn postgres_secret_slot() -> SecretSlot {
    SecretSlot::optional("database", ["postgres"], "PostgreSQL database credential.")
}

pub(super) fn database_effects() -> Vec<CommandEffect> {
    vec![
        CommandEffect::FilesystemRead,
        CommandEffect::FilesystemWrite,
        CommandEffect::ConfigurationRead,
        CommandEffect::ProcessSpawn,
        CommandEffect::NetworkRead,
        CommandEffect::ExternalRead,
    ]
}
