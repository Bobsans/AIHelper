use std::path::{Path, PathBuf};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    GlobalOptionsWire, Reversibility, RiskLevel, TypedInvocationRequest, TypedInvocationResponse,
};
use serde_json::{Map, Value, json};

use super::*;

pub(super) fn command_catalog() -> CommandCatalog {
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

pub(super) fn invoke(request: &TypedInvocationRequest) -> TypedInvocationResponse {
    let cli = match typed_cli(request) {
        Ok(cli) => cli,
        Err(error) => return TypedInvocationResponse::error(error),
    };
    let globals = GlobalOptionsWire {
        json: true,
        quiet: false,
        limit: request.context.limit,
    };
    invocation_response(request, execute(cli, &globals))
}

pub(super) fn cancel(_request_id: &str) -> bool {
    false
}

fn typed_cli(request: &TypedInvocationRequest) -> Result<PostgresCli, CommandError> {
    let arguments = &request.arguments;
    let cwd = Path::new(&request.context.cwd);
    let tool = ToolResolverArgs {
        tool_path: optional_path(arguments, "tool_path", cwd),
        ensure_tool: bool_or(arguments, "ensure_tool", false),
    };
    let connection = typed_connection(request);
    let command = match request.command.as_str() {
        "postgres.tool.status" => PostgresCommand::Tool(ToolArgs {
            command: ToolCommand::Status,
        }),
        "postgres.tool.download" => PostgresCommand::Tool(ToolArgs {
            command: ToolCommand::Download(ToolDownloadArgs {
                version: string_or(arguments, "version", DEFAULT_POSTGRES_VERSION),
                force: bool_or(arguments, "force", false),
                download_timeout_secs: u64_or(
                    arguments,
                    "timeout_secs",
                    DEFAULT_DOWNLOAD_TIMEOUT_SECS,
                )
                .min(remaining_seconds(request)),
            }),
        }),
        "postgres.tool.use" => PostgresCommand::Tool(ToolArgs {
            command: ToolCommand::Use(ToolUseArgs {
                path: required_path(arguments, "path", cwd, request)?,
            }),
        }),
        "postgres.tool.cleanup" => PostgresCommand::Tool(ToolArgs {
            command: ToolCommand::Cleanup(ToolCleanupArgs {
                version: optional_string(arguments, "version"),
            }),
        }),
        "postgres.ping" => PostgresCommand::Ping,
        "postgres.info" => PostgresCommand::Info,
        "postgres.databases" => PostgresCommand::Databases,
        "postgres.schemas" => PostgresCommand::Schemas(IncludeSystemArgs {
            include_system: bool_or(arguments, "include_system", false),
        }),
        "postgres.tables" => PostgresCommand::Tables(RelationListArgs {
            schema: optional_string(arguments, "schema"),
            include_system: bool_or(arguments, "include_system", false),
        }),
        "postgres.views" => PostgresCommand::Views(RelationListArgs {
            schema: optional_string(arguments, "schema"),
            include_system: bool_or(arguments, "include_system", false),
        }),
        "postgres.describe" => PostgresCommand::Describe(DescribeArgs {
            object: required_string(arguments, "object", request)?,
        }),
        "postgres.indexes" => PostgresCommand::Indexes(IndexesArgs {
            schema: optional_string(arguments, "schema"),
            table: optional_string(arguments, "table"),
        }),
        "postgres.extensions" => PostgresCommand::Extensions(ExtensionsArgs {
            available: bool_or(arguments, "available", false),
        }),
        "postgres.query" => PostgresCommand::Query(QueryArgs {
            sql: optional_string(arguments, "sql"),
            file: optional_path(arguments, "file", cwd),
        }),
        "postgres.exec" => PostgresCommand::Exec(ExecArgs {
            sql: optional_string(arguments, "sql"),
            file: optional_path(arguments, "file", cwd),
            single_transaction: bool_or(arguments, "single_transaction", false),
            yes: bool_or(arguments, "yes", false),
        }),
        "postgres.explain" => PostgresCommand::Explain(ExplainArgs {
            sql: optional_string(arguments, "sql"),
            file: optional_path(arguments, "file", cwd),
            analyze: bool_or(arguments, "analyze", false),
            buffers: bool_or(arguments, "buffers", false),
            yes: bool_or(arguments, "yes", false),
        }),
        "postgres.activity" => PostgresCommand::Activity(ActivityArgs {
            active: bool_or(arguments, "active", false),
            idle_in_tx: bool_or(arguments, "idle_in_tx", false),
        }),
        "postgres.locks" => PostgresCommand::Locks(LocksArgs {
            blocking: bool_or(arguments, "blocking", false),
        }),
        "postgres.size" => PostgresCommand::Size(SizeArgs {
            schema: optional_string(arguments, "schema"),
            table: optional_string(arguments, "table"),
        }),
        "postgres.settings" => PostgresCommand::Settings(SettingsArgs {
            changed: bool_or(arguments, "changed", false),
        }),
        _ => {
            return Err(command_error(
                request,
                "TYPED_COMMAND_NOT_FOUND",
                "Unknown PostgreSQL command",
                "the command is not present in the PostgreSQL typed catalog",
                false,
            ));
        }
    };
    Ok(PostgresCli {
        tool,
        connection,
        command,
    })
}

fn typed_connection(request: &TypedInvocationRequest) -> ConnectionArgs {
    let arguments = &request.arguments;
    let remaining_ms = request.context.remaining_timeout_ms.max(1);
    ConnectionArgs {
        host: optional_string(arguments, "host"),
        port: arguments
            .get("port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok()),
        database: optional_string(arguments, "database"),
        user: optional_string(arguments, "user"),
        service: optional_string(arguments, "service"),
        sslmode: optional_string(arguments, "sslmode"),
        password_env: optional_string(arguments, "password_env"),
        connect_timeout_secs: u64_or(
            arguments,
            "connect_timeout_secs",
            DEFAULT_CONNECT_TIMEOUT_SECS,
        )
        .min(remaining_seconds(request)),
        statement_timeout_ms: Some(
            arguments
                .get("statement_timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(remaining_ms)
                .max(1)
                .min(remaining_ms),
        ),
    }
}

fn remaining_seconds(request: &TypedInvocationRequest) -> u64 {
    request
        .context
        .remaining_timeout_ms
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or(1)
        .max(1)
}

fn invocation_response(
    request: &TypedInvocationRequest,
    response: InvocationResponse,
) -> TypedInvocationResponse {
    if !response.success {
        if let Some(diagnostic) = response.diagnostic {
            return TypedInvocationResponse::error(CommandError::from_diagnostic(
                diagnostic
                    .with_domain(DOMAIN)
                    .with_operation(request.command.clone()),
                retryable_code(
                    response
                        .error_code
                        .as_deref()
                        .unwrap_or("POSTGRES_REQUEST_FAILED"),
                ),
            ));
        }
        let code = response
            .error_code
            .unwrap_or_else(|| "POSTGRES_REQUEST_FAILED".to_owned());
        let message = response
            .error_message
            .unwrap_or_else(|| "PostgreSQL command failed".to_owned());
        return TypedInvocationResponse::error(command_error(
            request,
            &code,
            &message,
            &message,
            retryable_code(&code),
        ));
    }
    let Some(raw) = response.message else {
        return TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "PostgreSQL command returned no structured output",
            "the shared command implementation omitted its JSON result",
            false,
        ));
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(data) if data.is_object() => {
            TypedInvocationResponse::success(data, Some(format!("Completed {}.", request.command)))
        }
        Ok(_) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "PostgreSQL command returned non-object output",
            "typed commands require a JSON object result",
            false,
        )),
        Err(error) => TypedInvocationResponse::error(command_error(
            request,
            "INVALID_TYPED_RESPONSE",
            "Failed to decode PostgreSQL command output",
            error.to_string(),
            false,
        )),
    }
}

fn retryable_code(code: &str) -> bool {
    code.contains("PSQL")
        || code.contains("TIMEOUT")
        || code.contains("DOWNLOAD")
        || code.contains("LOCKED")
}

fn command_error(
    request: &TypedInvocationRequest,
    code: impl Into<String>,
    message: impl Into<String>,
    cause: impl Into<String>,
    retryable: bool,
) -> CommandError {
    CommandError::new(
        Some(DOMAIN.to_owned()),
        Some(request.command.clone()),
        code,
        message,
        cause,
        1,
        retryable,
    )
}

fn required_string(
    arguments: &Value,
    name: &str,
    request: &TypedInvocationRequest,
) -> Result<String, CommandError> {
    optional_string(arguments, name)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            command_error(
                request,
                "INVALID_ARGUMENT",
                format!("Missing {name}"),
                format!("typed input requires non-empty '{name}'"),
                false,
            )
        })
}

fn optional_string(arguments: &Value, name: &str) -> Option<String> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn string_or(arguments: &Value, name: &str, default: &str) -> String {
    optional_string(arguments, name).unwrap_or_else(|| default.to_owned())
}

fn bool_or(arguments: &Value, name: &str, default: bool) -> bool {
    arguments
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn u64_or(arguments: &Value, name: &str, default: u64) -> u64 {
    arguments
        .get(name)
        .and_then(Value::as_u64)
        .unwrap_or(default)
        .max(1)
}

fn optional_path(arguments: &Value, name: &str, cwd: &Path) -> Option<PathBuf> {
    optional_string(arguments, name).map(|value| absolute_path(cwd, &value))
}

fn required_path(
    arguments: &Value,
    name: &str,
    cwd: &Path,
    request: &TypedInvocationRequest,
) -> Result<PathBuf, CommandError> {
    required_string(arguments, name, request).map(|value| absolute_path(cwd, &value))
}

fn absolute_path(cwd: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

fn tool_status_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.status",
        "Inspect PostgreSQL toolchain",
        "Resolve psql candidates and report the selected client toolchain.",
        tool_path_input(),
        tool_status_output(),
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

fn tool_download_descriptor() -> CommandDescriptor {
    let properties = Map::from_iter([
        (
            "version".to_owned(),
            json!({
                "type": "string",
                "minLength": 1,
                "default": DEFAULT_POSTGRES_VERSION
            }),
        ),
        (
            "force".to_owned(),
            boolean_schema(false, "Replace an existing managed toolchain."),
        ),
        (
            "timeout_secs".to_owned(),
            positive_integer_default(
                DEFAULT_DOWNLOAD_TIMEOUT_SECS,
                "Download timeout capped by the MCP request deadline.",
            ),
        ),
    ]);
    descriptor(
        "postgres.tool.download",
        "Download PostgreSQL toolchain",
        "Download, verify, and unpack the supported PostgreSQL client toolchain.",
        object_input(properties, Vec::new()),
        top_output(
            "postgres.tool.download",
            &[
                ("version", string_schema()),
                ("url", string_schema()),
                ("sha256", string_schema()),
                ("cache_path", string_schema()),
                ("bin_dir", string_schema()),
                ("psql_path", string_schema()),
                ("downloaded", boolean_value_schema()),
            ],
        ),
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

fn tool_use_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.use",
        "Select PostgreSQL toolchain",
        "Validate a psql toolchain path and persist it as the shared default.",
        object_input(
            Map::from_iter([(
                "path".to_owned(),
                required_text_schema("Tool executable or directory resolved against cwd."),
            )]),
            vec!["path"],
        ),
        top_output(
            "postgres.tool.use",
            &[
                ("path", string_schema()),
                ("psql_path", string_schema()),
                ("version", tool_version_schema()),
                ("config_path", string_schema()),
            ],
        ),
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

fn tool_cleanup_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.tool.cleanup",
        "Clean PostgreSQL tool cache",
        "Delete one version or all versions from the managed PostgreSQL tool cache.",
        object_input(
            Map::from_iter([(
                "version".to_owned(),
                optional_text_schema(
                    "Managed version to remove; omit to remove every cached version.",
                ),
            )]),
            Vec::new(),
        ),
        top_output(
            "postgres.tool.cleanup",
            &[(
                "removed",
                json!({"type": "array", "items": string_schema()}),
            )],
        ),
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

fn ping_descriptor() -> CommandDescriptor {
    database_descriptor(
        "postgres.ping",
        "Ping PostgreSQL",
        "Connect and return server plus session identity metadata.",
        Map::new(),
        info_output("postgres.ping"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

fn info_descriptor() -> CommandDescriptor {
    database_descriptor(
        "postgres.info",
        "Inspect PostgreSQL session",
        "Return selected server and session metadata.",
        Map::new(),
        info_output("postgres.info"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

fn databases_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.databases",
        "List PostgreSQL databases",
        Map::new(),
        database_row_schema(),
        "Reads database names, owners, encodings, connection flags, and visible size information.",
    )
}

fn schemas_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.schemas",
        "List PostgreSQL schemas",
        Map::from_iter([(
            "include_system".to_owned(),
            boolean_schema(false, "Include system schemas."),
        )]),
        schema_row_schema(),
        "Reads schema names and owners, optionally including system schemas.",
    )
}

fn relations_descriptor(id: &str, title: &str) -> CommandDescriptor {
    rows_descriptor(
        id,
        title,
        Map::from_iter([
            (
                "schema".to_owned(),
                optional_text_schema("Restrict results to this schema."),
            ),
            (
                "include_system".to_owned(),
                boolean_schema(false, "Include system relations."),
            ),
        ]),
        relation_row_schema(),
        "Reads relation names, owners, row estimates, and total sizes.",
    )
}

fn describe_descriptor() -> CommandDescriptor {
    database_descriptor(
        "postgres.describe",
        "Describe PostgreSQL relation",
        "Describe a table, view, or materialized view.",
        Map::from_iter([(
            "object".to_owned(),
            required_text_schema("Relation name as NAME or SCHEMA.NAME."),
        )]),
        top_output(
            "postgres.describe",
            &[
                ("relation", describe_relation_schema()),
                (
                    "columns",
                    json!({"type": "array", "items": column_row_schema()}),
                ),
                (
                    "indexes",
                    json!({"type": "array", "items": index_row_schema()}),
                ),
                (
                    "constraints",
                    json!({"type": "array", "items": constraint_row_schema()}),
                ),
            ],
        ),
        "Reads relation, column, index, and constraint definitions that may reveal database structure.",
    )
}

fn indexes_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.indexes",
        "List PostgreSQL indexes",
        Map::from_iter([
            (
                "schema".to_owned(),
                optional_text_schema("Restrict results to this schema."),
            ),
            (
                "table".to_owned(),
                optional_text_schema("Restrict results to this table."),
            ),
        ]),
        index_row_schema(),
        "Reads index names and full definitions.",
    )
}

fn extensions_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.extensions",
        "List PostgreSQL extensions",
        Map::from_iter([(
            "available".to_owned(),
            boolean_schema(false, "Include available but not installed extensions."),
        )]),
        extension_row_schema(),
        "Reads installed extension metadata or the server's available extension catalog.",
    )
}

fn query_descriptor() -> CommandDescriptor {
    descriptor(
        "postgres.query",
        "Query PostgreSQL",
        "Run SQL restricted to SELECT, WITH, TABLE, or VALUES in a read-only transaction.",
        database_input(sql_source_properties(), vec![]),
        top_output(
            "postgres.query",
            &[
                ("row_count", nonnegative_integer_schema()),
                ("rows", json!({"type": "array", "items": {}})),
            ],
        ),
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
    .with_example(CommandExample::new(
        "Read the current time",
        json!({"sql": "select now() as current_time"}),
    ))
}

fn exec_descriptor() -> CommandDescriptor {
    let mut properties = sql_source_properties();
    properties.insert(
        "single_transaction".to_owned(),
        boolean_schema(false, "Execute all SQL in one transaction."),
    );
    properties.insert(
        "yes".to_owned(),
        json!({
            "type": "boolean",
            "const": true,
            "description": "Required explicit confirmation for arbitrary SQL mutation."
        }),
    );
    descriptor(
        "postgres.exec",
        "Execute PostgreSQL SQL",
        "Execute explicitly confirmed SQL mutations or administrative commands.",
        database_input(properties, vec!["yes"]),
        top_output(
            "postgres.exec",
            &[("stdout", string_schema()), ("stderr", string_schema())],
        ),
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
}

fn explain_descriptor() -> CommandDescriptor {
    let mut properties = sql_source_properties();
    properties.insert(
        "analyze".to_owned(),
        boolean_schema(
            false,
            "Execute the SQL while collecting actual plan statistics.",
        ),
    );
    properties.insert(
        "buffers".to_owned(),
        boolean_schema(false, "Include buffer usage in the plan."),
    );
    properties.insert(
        "yes".to_owned(),
        boolean_schema(
            false,
            "Required when analyze=true because the SQL is executed.",
        ),
    );
    let schema = database_input(properties, Vec::new());
    descriptor(
        "postgres.explain",
        "Explain PostgreSQL SQL",
        "Return a query plan; analyze=true executes the supplied SQL.",
        schema,
        top_output(
            "postgres.explain",
            &[
                ("analyze", boolean_value_schema()),
                ("buffers", boolean_value_schema()),
                ("plan", json!({})),
            ],
        ),
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
}

fn activity_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.activity",
        "Inspect PostgreSQL activity",
        Map::from_iter([
            (
                "active".to_owned(),
                boolean_schema(false, "Show only active sessions."),
            ),
            (
                "idle_in_tx".to_owned(),
                boolean_schema(false, "Show only sessions idle in a transaction."),
            ),
        ]),
        activity_row_schema(),
        "Reads session identities, client addresses, wait states, timestamps, and truncated SQL text from pg_stat_activity.",
    )
}

fn locks_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.locks",
        "Inspect PostgreSQL locks",
        Map::from_iter([(
            "blocking".to_owned(),
            boolean_schema(false, "Return only locks with a known blocking session."),
        )]),
        lock_row_schema(),
        "Reads blocked and blocking session identities plus truncated SQL text.",
    )
}

fn size_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.size",
        "Inspect PostgreSQL sizes",
        Map::from_iter([
            (
                "schema".to_owned(),
                optional_text_schema("Show aggregate size for a schema or qualify a table."),
            ),
            (
                "table".to_owned(),
                optional_text_schema("Show table size; may be NAME or SCHEMA.NAME."),
            ),
        ]),
        size_row_schema(),
        "Reads database, schema, or relation size statistics.",
    )
}

fn settings_descriptor() -> CommandDescriptor {
    rows_descriptor(
        "postgres.settings",
        "Inspect PostgreSQL settings",
        Map::from_iter([(
            "changed".to_owned(),
            boolean_schema(false, "Return only settings whose source is not default."),
        )]),
        setting_row_schema(),
        "Reads server settings, sources, units, and descriptions; configuration values may contain sensitive operational details.",
    )
}

fn rows_descriptor(
    id: &str,
    title: &str,
    properties: Map<String, Value>,
    row_schema: Value,
    impact: &str,
) -> CommandDescriptor {
    database_descriptor(
        id,
        title,
        title,
        properties,
        rows_output(id, row_schema),
        impact,
    )
}

fn database_descriptor(
    id: &str,
    title: &str,
    description: &str,
    properties: Map<String, Value>,
    output_schema: Value,
    impact: &str,
) -> CommandDescriptor {
    descriptor(
        id,
        title,
        description,
        database_input(properties, Vec::new()),
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
}

fn descriptor(
    id: &str,
    title: &str,
    description: &str,
    input_schema: Value,
    output_schema: Value,
    effects: CommandEffects,
) -> CommandDescriptor {
    CommandDescriptor::new(id, title, description, input_schema, output_schema, effects)
}

fn database_effects() -> Vec<CommandEffect> {
    vec![
        CommandEffect::FilesystemRead,
        CommandEffect::FilesystemWrite,
        CommandEffect::ConfigurationRead,
        CommandEffect::ProcessSpawn,
        CommandEffect::NetworkRead,
        CommandEffect::ExternalRead,
    ]
}

fn tool_path_input() -> Value {
    object_input(
        Map::from_iter([(
            "tool_path".to_owned(),
            optional_text_schema(
                "Explicit psql executable or tool directory resolved against cwd.",
            ),
        )]),
        Vec::new(),
    )
}

fn database_input(mut properties: Map<String, Value>, required: Vec<&str>) -> Value {
    properties.insert(
        "tool_path".to_owned(),
        optional_text_schema("Explicit psql executable or tool directory resolved against cwd."),
    );
    properties.insert(
        "ensure_tool".to_owned(),
        boolean_schema(
            false,
            "Download the managed toolchain when no usable psql exists. This writes the shared cache.",
        ),
    );
    properties.insert("host".to_owned(), optional_text_schema("PostgreSQL host."));
    properties.insert(
        "port".to_owned(),
        json!({"type": "integer", "minimum": 1, "maximum": 65535}),
    );
    properties.insert(
        "database".to_owned(),
        optional_text_schema("PostgreSQL database name."),
    );
    properties.insert("user".to_owned(), optional_text_schema("PostgreSQL user."));
    properties.insert(
        "service".to_owned(),
        optional_text_schema("libpq service name."),
    );
    properties.insert(
        "sslmode".to_owned(),
        json!({
            "type": "string",
            "enum": ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"]
        }),
    );
    properties.insert(
        "password_env".to_owned(),
        optional_text_schema("Environment variable whose value is passed to psql as PGPASSWORD."),
    );
    properties.insert(
        "connect_timeout_secs".to_owned(),
        positive_integer_default(
            DEFAULT_CONNECT_TIMEOUT_SECS,
            "Connection timeout capped by the MCP request deadline.",
        ),
    );
    properties.insert(
        "statement_timeout_ms".to_owned(),
        json!({
            "type": "integer",
            "minimum": 1,
            "description": "Server statement timeout capped by the MCP request deadline."
        }),
    );
    object_input(properties, required)
}

fn sql_source_properties() -> Map<String, Value> {
    Map::from_iter([
        (
            "sql".to_owned(),
            optional_text_schema("Inline SQL text. Exactly one of sql or file is required."),
        ),
        (
            "file".to_owned(),
            optional_text_schema(
                "UTF-8 SQL file resolved against cwd. Exactly one of sql or file is required.",
            ),
        ),
    ])
}

fn object_input(properties: Map<String, Value>, required: Vec<&str>) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn required_text_schema(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}

fn optional_text_schema(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn boolean_schema(default: bool, description: &str) -> Value {
    json!({"type": "boolean", "default": default, "description": description})
}

fn positive_integer_default(default: u64, description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": default,
        "description": description
    })
}

fn string_schema() -> Value {
    json!({"type": "string"})
}

fn boolean_value_schema() -> Value {
    json!({"type": "boolean"})
}

fn integer_schema() -> Value {
    json!({"type": "integer"})
}

fn positive_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 1})
}

fn nonnegative_integer_schema() -> Value {
    json!({"type": "integer", "minimum": 0})
}

fn nullable(schema: Value) -> Value {
    json!({"oneOf": [schema, {"type": "null"}]})
}

fn exact_object(fields: &[(&str, Value)]) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for (name, schema) in fields {
        properties.insert((*name).to_owned(), schema.clone());
        required.push(*name);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn top_output(command: &str, fields: &[(&str, Value)]) -> Value {
    let mut all_fields = vec![("command", json!({"type": "string", "const": command}))];
    all_fields.extend(fields.iter().cloned());
    exact_object(&all_fields)
}

fn rows_output(command: &str, row_schema: Value) -> Value {
    top_output(
        command,
        &[
            ("count", nonnegative_integer_schema()),
            ("rows", json!({"type": "array", "items": row_schema})),
        ],
    )
}

fn info_output(command: &str) -> Value {
    top_output(
        command,
        &[
            ("server_version", string_schema()),
            ("current_database", string_schema()),
            ("current_user", string_schema()),
            ("session_user", string_schema()),
            ("current_schema", nullable(string_schema())),
            ("server_encoding", string_schema()),
            ("inet_server_addr", nullable(string_schema())),
            ("inet_server_port", nullable(integer_schema())),
        ],
    )
}

fn candidate_schema() -> Value {
    exact_object(&[
        ("source", string_schema()),
        ("path", string_schema()),
        ("psql_path", nullable(string_schema())),
        ("bin_dir", nullable(string_schema())),
        ("version_raw", nullable(string_schema())),
        ("version_major", nullable(positive_integer_schema())),
        ("accepted", boolean_value_schema()),
        ("reason", nullable(string_schema())),
    ])
}

fn tool_version_schema() -> Value {
    exact_object(&[
        ("raw", string_schema()),
        ("major", nullable(positive_integer_schema())),
    ])
}

fn tool_status_output() -> Value {
    top_output(
        "postgres.tool.status",
        &[
            ("available", boolean_value_schema()),
            ("selected", nullable(candidate_schema())),
            (
                "candidates",
                json!({"type": "array", "items": candidate_schema()}),
            ),
            ("target_version", string_schema()),
            ("minimum_major", positive_integer_schema()),
            ("cache_dir", nullable(string_schema())),
            ("config_path", nullable(string_schema())),
            ("remediation", nullable(string_schema())),
        ],
    )
}

fn database_row_schema() -> Value {
    exact_object(&[
        ("name", string_schema()),
        ("owner", string_schema()),
        ("encoding", string_schema()),
        ("allow_connections", boolean_value_schema()),
        ("size", nullable(string_schema())),
    ])
}

fn schema_row_schema() -> Value {
    exact_object(&[("name", string_schema()), ("owner", string_schema())])
}

fn relation_row_schema() -> Value {
    exact_object(&[
        ("schema", string_schema()),
        ("name", string_schema()),
        ("kind", string_schema()),
        ("owner", string_schema()),
        ("rows_estimate", nullable(integer_schema())),
        ("size", string_schema()),
    ])
}

fn column_row_schema() -> Value {
    exact_object(&[
        ("ordinal", integer_schema()),
        ("name", string_schema()),
        ("data_type", string_schema()),
        ("nullable", boolean_value_schema()),
        ("default", nullable(string_schema())),
        ("comment", nullable(string_schema())),
    ])
}

fn index_row_schema() -> Value {
    exact_object(&[
        ("schema", string_schema()),
        ("table", string_schema()),
        ("name", string_schema()),
        ("primary", boolean_value_schema()),
        ("unique", boolean_value_schema()),
        ("definition", string_schema()),
    ])
}

fn constraint_row_schema() -> Value {
    exact_object(&[
        ("name", string_schema()),
        ("constraint_type", string_schema()),
        ("definition", string_schema()),
    ])
}

fn describe_relation_schema() -> Value {
    exact_object(&[
        ("schema", string_schema()),
        ("name", string_schema()),
        ("kind", string_schema()),
        ("owner", string_schema()),
        ("rows_estimate", nullable(integer_schema())),
        ("total_size", string_schema()),
    ])
}

fn extension_row_schema() -> Value {
    exact_object(&[
        ("name", string_schema()),
        ("installed_version", nullable(string_schema())),
        ("default_version", nullable(string_schema())),
        ("schema", nullable(string_schema())),
        ("comment", nullable(string_schema())),
    ])
}

fn activity_row_schema() -> Value {
    exact_object(&[
        ("pid", integer_schema()),
        ("user", nullable(string_schema())),
        ("database", nullable(string_schema())),
        ("application_name", nullable(string_schema())),
        ("client_addr", nullable(string_schema())),
        ("state", nullable(string_schema())),
        ("wait_event_type", nullable(string_schema())),
        ("wait_event", nullable(string_schema())),
        ("query_start", nullable(string_schema())),
        ("state_change", nullable(string_schema())),
        ("query", nullable(string_schema())),
    ])
}

fn lock_row_schema() -> Value {
    exact_object(&[
        ("blocked_pid", integer_schema()),
        ("blocked_user", nullable(string_schema())),
        ("blocking_pid", nullable(integer_schema())),
        ("blocking_user", nullable(string_schema())),
        ("lock_type", string_schema()),
        ("mode", string_schema()),
        ("relation", nullable(string_schema())),
        ("blocked_query", nullable(string_schema())),
        ("blocking_query", nullable(string_schema())),
    ])
}

fn size_row_schema() -> Value {
    exact_object(&[
        ("scope", string_schema()),
        ("schema", nullable(string_schema())),
        ("name", string_schema()),
        ("size", string_schema()),
        ("bytes", integer_schema()),
    ])
}

fn setting_row_schema() -> Value {
    exact_object(&[
        ("name", string_schema()),
        ("setting", string_schema()),
        ("unit", nullable(string_schema())),
        ("source", string_schema()),
        ("short_desc", string_schema()),
    ])
}

#[cfg(test)]
mod tests {
    use ah_plugin_api::ExecutionContextWire;

    use super::*;

    #[test]
    fn catalog_contains_all_postgres_commands() {
        let catalog = command_catalog();
        assert_eq!(catalog.commands.len(), 20);
        assert!(catalog.commands.iter().all(|command| {
            command.input_schema["type"] == "object" && command.output_schema["type"] == "object"
        }));
        assert!(catalog.commands.iter().all(|command| {
            let root = command.input_schema.as_object().unwrap();
            ["oneOf", "anyOf", "allOf", "not", "if", "then", "else"]
                .iter()
                .all(|keyword| !root.contains_key(*keyword))
        }));
        assert!(
            catalog
                .commands
                .iter()
                .any(|item| item.id == "postgres.exec")
        );
        assert!(
            catalog
                .commands
                .iter()
                .any(|item| item.id == "postgres.tool.cleanup")
        );
    }

    #[test]
    fn typed_paths_and_timeouts_use_request_context() {
        let request = TypedInvocationRequest::new(
            "postgres.query",
            json!({
                "file": "queries/check.sql",
                "connect_timeout_secs": 60,
                "statement_timeout_ms": 60_000
            }),
            ExecutionContextWire::new("request-1", "C:/workspace", Some(10), 1_200),
        );

        let cli = typed_cli(&request).expect("typed command should parse");
        let PostgresCommand::Query(args) = cli.command else {
            panic!("expected query command");
        };
        assert_eq!(
            args.file,
            Some(PathBuf::from("C:/workspace").join("queries/check.sql"))
        );
        assert_eq!(cli.connection.connect_timeout_secs, 2);
        assert_eq!(cli.connection.statement_timeout_ms, Some(1_200));
    }

    #[test]
    fn exec_requires_explicit_confirmation_in_schema() {
        let descriptor = exec_descriptor();
        assert_eq!(descriptor.input_schema["properties"]["yes"]["const"], true);
        assert!(descriptor.effects.destructive);
        assert_eq!(descriptor.effects.risk, RiskLevel::Critical);
    }
}
