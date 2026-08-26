use std::path::{Path, PathBuf};

use ah_plugin_api::{
    CommandCatalog, CommandDescriptor, CommandEffect, CommandEffects, CommandError, CommandExample,
    GlobalOptionsWire, InvocationResponse, Reversibility, RiskLevel, SecretSlot,
    TypedInvocationRequest, TypedInvocationResponse,
    schema::{input_schema_for, output_schema_for},
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;

// A database command takes the toolchain block, the connection block and its own
// arguments; a tool command takes only the toolchain block. A doc comment on
// either would be published as the schema `description`, and
// `deny_unknown_fields` cannot be combined with `flatten`, so the closed-object
// rule is stated for the schema and the runtime enforces it before dispatch.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
struct DbWire<T> {
    #[serde(flatten)]
    tool: ToolResolverArgs,
    #[serde(flatten)]
    connection: ConnectionArgs,
    #[serde(flatten)]
    command: T,
}

// A command whose only arguments are the shared blocks.
#[derive(Debug, Deserialize, JsonSchema)]
struct NoArgs {}

/// Arguments are validated against the derived input schema before dispatch, so
/// a failure here means the schema and the type disagree.
fn decode<T: serde::de::DeserializeOwned>(
    request: &TypedInvocationRequest,
) -> Result<T, CommandError> {
    serde_json::from_value(request.arguments.clone()).map_err(|error| {
        command_error(
            request,
            "INVALID_ARGUMENT",
            "PostgreSQL command arguments are invalid",
            error.to_string(),
            false,
        )
    })
}

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
        cwd: None,
    };
    invocation_response(request, execute(cli, &globals))
}

pub(super) fn cancel(_request_id: &str) -> bool {
    false
}

fn typed_cli(request: &TypedInvocationRequest) -> Result<PostgresCli, CommandError> {
    let cwd = Path::new(&request.context.cwd);

    /// A toolchain command: no connection block, no credential, and no
    /// `ensure_tool` - see [`ToolPathArgs`].
    macro_rules! tool_command {
        ($args:ty) => {{
            let args: $args = decode(request)?;
            (
                ToolResolverArgs {
                    tool_path: None,
                    ensure_tool: false,
                },
                ConnectionArgs::unset(),
                args,
            )
        }};
    }

    /// A database command: toolchain plus connection.
    macro_rules! db_command {
        ($args:ty) => {{
            let wire: DbWire<$args> = decode(request)?;
            (wire.tool, wire.connection, wire.command)
        }};
    }

    let (mut tool, connection, command) = match request.command.as_str() {
        "postgres.tool.status" => {
            let args: ToolPathArgs = decode(request)?;
            (
                ToolResolverArgs {
                    tool_path: args.tool_path,
                    ensure_tool: false,
                },
                ConnectionArgs::unset(),
                PostgresCommand::Tool(ToolArgs {
                    command: ToolCommand::Status,
                }),
            )
        }
        "postgres.tool.download" => {
            let (tool, connection, mut args) = tool_command!(ToolDownloadArgs);
            args.download_timeout_secs = args.download_timeout_secs.min(remaining_seconds(request));
            (
                tool,
                connection,
                PostgresCommand::Tool(ToolArgs {
                    command: ToolCommand::Download(args),
                }),
            )
        }
        "postgres.tool.use" => {
            let (tool, connection, mut args) = tool_command!(ToolUseArgs);
            args.path = resolve_path(args.path, cwd);
            (
                tool,
                connection,
                PostgresCommand::Tool(ToolArgs {
                    command: ToolCommand::Use(args),
                }),
            )
        }
        "postgres.tool.cleanup" => {
            let (tool, connection, args) = tool_command!(ToolCleanupArgs);
            (
                tool,
                connection,
                PostgresCommand::Tool(ToolArgs {
                    command: ToolCommand::Cleanup(args),
                }),
            )
        }
        "postgres.ping" => {
            let (tool, connection, _) = db_command!(NoArgs);
            (tool, connection, PostgresCommand::Ping)
        }
        "postgres.info" => {
            let (tool, connection, _) = db_command!(NoArgs);
            (tool, connection, PostgresCommand::Info)
        }
        "postgres.databases" => {
            let (tool, connection, _) = db_command!(NoArgs);
            (tool, connection, PostgresCommand::Databases)
        }
        "postgres.schemas" => {
            let (tool, connection, args) = db_command!(IncludeSystemArgs);
            (tool, connection, PostgresCommand::Schemas(args))
        }
        "postgres.tables" => {
            let (tool, connection, args) = db_command!(RelationListArgs);
            (tool, connection, PostgresCommand::Tables(args))
        }
        "postgres.views" => {
            let (tool, connection, args) = db_command!(RelationListArgs);
            (tool, connection, PostgresCommand::Views(args))
        }
        "postgres.describe" => {
            let (tool, connection, args) = db_command!(DescribeArgs);
            (tool, connection, PostgresCommand::Describe(args))
        }
        "postgres.indexes" => {
            let (tool, connection, args) = db_command!(IndexesArgs);
            (tool, connection, PostgresCommand::Indexes(args))
        }
        "postgres.extensions" => {
            let (tool, connection, args) = db_command!(ExtensionsArgs);
            (tool, connection, PostgresCommand::Extensions(args))
        }
        "postgres.query" => {
            let (tool, connection, mut args) = db_command!(QueryArgs);
            args.file = args.file.map(|file| resolve_path(file, cwd));
            (tool, connection, PostgresCommand::Query(args))
        }
        "postgres.exec" => {
            let (tool, connection, mut args) = db_command!(ExecArgs);
            args.file = args.file.map(|file| resolve_path(file, cwd));
            (tool, connection, PostgresCommand::Exec(args))
        }
        "postgres.explain" => {
            let (tool, connection, mut args) = db_command!(ExplainArgs);
            args.file = args.file.map(|file| resolve_path(file, cwd));
            (tool, connection, PostgresCommand::Explain(args))
        }
        "postgres.activity" => {
            let (tool, connection, args) = db_command!(ActivityArgs);
            (tool, connection, PostgresCommand::Activity(args))
        }
        "postgres.locks" => {
            let (tool, connection, args) = db_command!(LocksArgs);
            (tool, connection, PostgresCommand::Locks(args))
        }
        "postgres.size" => {
            let (tool, connection, args) = db_command!(SizeArgs);
            (tool, connection, PostgresCommand::Size(args))
        }
        "postgres.settings" => {
            let (tool, connection, args) = db_command!(SettingsArgs);
            (tool, connection, PostgresCommand::Settings(args))
        }
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

    tool.tool_path = tool.tool_path.map(|path| resolve_path(path, cwd));
    Ok(PostgresCli {
        tool,
        connection: apply_context(connection, request)?,
        command,
    })
}

/// A relative path argument is resolved against the execution cwd.
fn resolve_path(path: PathBuf, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Fill in what the caller cannot supply: the credential from the vault, and the
/// timeouts bounded by the request deadline.
fn apply_context(
    mut connection: ConnectionArgs,
    request: &TypedInvocationRequest,
) -> Result<ConnectionArgs, CommandError> {
    connection.resolved_password = resolved_database_password(request)?;
    let unbounded = request.context.remaining_timeout_ms == u64::MAX;
    if !unbounded {
        let remaining_ms = request.context.remaining_timeout_ms.max(1);
        connection.connect_timeout_secs = connection
            .connect_timeout_secs
            .min(remaining_seconds(request));
        connection.statement_timeout_ms = Some(
            connection
                .statement_timeout_ms
                .unwrap_or(remaining_ms)
                .max(1)
                .min(remaining_ms),
        );
    }
    Ok(connection)
}

fn resolved_database_password(
    request: &TypedInvocationRequest,
) -> Result<Option<SecretValue>, CommandError> {
    let credentials = request
        .arguments
        .get("credentials")
        .and_then(Value::as_object);
    if let Some(slot) = credentials.and_then(|items| items.keys().find(|key| *key != "database")) {
        return Err(command_error(
            request,
            "INVALID_ARGUMENT",
            format!("Unsupported PostgreSQL credential slot '{slot}'"),
            "only the database credential slot is supported",
            false,
        ));
    }
    let public_id = credentials
        .and_then(|items| items.get("database"))
        .and_then(Value::as_str);
    if public_id.is_some()
        && request
            .arguments
            .get("password_env")
            .and_then(Value::as_str)
            .is_some()
    {
        return Err(command_error(
            request,
            "INVALID_ARGUMENT",
            "PostgreSQL database credential conflicts with password_env",
            "select either a vault credential or password_env",
            false,
        ));
    }
    let password = password_from_resolved_secrets(&request.resolved_secrets)
        .map_err(|error| invocation_to_command_error(request, error))?;
    match (public_id, &password) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(command_error(
                request,
                "SECRET_REQUIRED",
                "PostgreSQL database credential was not resolved",
                "public credential selection and private resolution must both be present",
                false,
            ));
        }
    }
    if let Some((public_id, secret)) = public_id.zip(request.resolved_secrets.get("database"))
        && secret.id != public_id
    {
        return Err(command_error(
            request,
            "SECRET_KIND_MISMATCH",
            "Resolved PostgreSQL database credential does not match the selected credential",
            "credential identity or kind mismatch",
            false,
        ));
    }
    Ok(password)
}

/// Shared by the typed and direct CLI paths: validates the slot and shape of the
/// credential the host resolved.
pub(super) fn password_from_resolved_secrets(
    secrets: &std::collections::BTreeMap<String, ah_plugin_api::ResolvedSecret>,
) -> Result<Option<SecretValue>, InvocationResponse> {
    if let Some(slot) = secrets.keys().find(|slot| slot.as_str() != "database") {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("Unsupported resolved PostgreSQL credential slot '{slot}'"),
        ));
    }
    let Some(resolved) = secrets.get("database") else {
        return Ok(None);
    };
    if resolved.kind != "postgres" {
        return Err(InvocationResponse::error(
            "SECRET_KIND_MISMATCH",
            "Resolved PostgreSQL database credential does not match the selected credential",
        ));
    }
    let password = resolved.values.get("password").ok_or_else(|| {
        InvocationResponse::error(
            "SECRET_REQUIRED",
            "Resolved PostgreSQL credential has no password",
        )
    })?;
    Ok(Some(SecretValue::new(password)))
}

fn invocation_to_command_error(
    request: &TypedInvocationRequest,
    response: InvocationResponse,
) -> CommandError {
    command_error(
        request,
        response
            .error_code
            .unwrap_or_else(|| "INVALID_ARGUMENT".to_owned()),
        response
            .error_message
            .unwrap_or_else(|| "credential resolution failed".to_owned()),
        "credential slot or kind mismatch",
        false,
    )
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
            let text = typed_text(&request.command, &data)
                .unwrap_or_else(|| format!("Completed {}.\n", request.command));
            TypedInvocationResponse::success(data, Some(text))
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

fn typed_text(command: &str, data: &Value) -> Option<String> {
    let formatter = TextFormatter::stdout();
    let rows = |data: &Value| {
        data.get("rows")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()))
    };
    match command {
        "postgres.ping" => serde_json::from_value::<InfoRow>(data.clone())
            .ok()
            .map(|row| render_ping_text(&row, formatter)),
        "postgres.info" => serde_json::from_value::<InfoRow>(data.clone())
            .ok()
            .map(|row| render_info_text(&row, formatter)),
        "postgres.databases" => serde_json::from_value::<Vec<DatabaseRow>>(rows(data))
            .ok()
            .map(|rows| render_database_rows(&rows, formatter)),
        "postgres.schemas" => serde_json::from_value::<Vec<SchemaRow>>(rows(data))
            .ok()
            .map(|rows| render_schema_rows(&rows, formatter)),
        "postgres.tables" | "postgres.views" => {
            serde_json::from_value::<Vec<RelationRow>>(rows(data))
                .ok()
                .map(|rows| render_relation_rows(&rows, formatter))
        }
        "postgres.describe" => serde_json::from_value::<DescribeOutput>(data.clone())
            .ok()
            .map(|output| render_describe_text(&output, formatter)),
        "postgres.indexes" => serde_json::from_value::<Vec<IndexRow>>(rows(data))
            .ok()
            .map(|rows| render_index_rows(&rows, formatter)),
        "postgres.extensions" => serde_json::from_value::<Vec<ExtensionRow>>(rows(data))
            .ok()
            .map(|rows| render_extension_rows(&rows, formatter)),
        "postgres.activity" => serde_json::from_value::<Vec<ActivityRow>>(rows(data))
            .ok()
            .map(|rows| render_activity_rows(&rows, formatter)),
        "postgres.locks" => serde_json::from_value::<Vec<LockRow>>(rows(data))
            .ok()
            .map(|rows| render_lock_rows(&rows, formatter)),
        "postgres.size" => serde_json::from_value::<Vec<SizeRow>>(rows(data))
            .ok()
            .map(|rows| render_size_rows(&rows, formatter)),
        "postgres.settings" => serde_json::from_value::<Vec<SettingRow>>(rows(data))
            .ok()
            .map(|rows| render_setting_rows(&rows, formatter)),
        "postgres.query" => data
            .get("rows")
            .and_then(|rows| serde_json::to_string_pretty(rows).ok()),
        "postgres.exec" => data
            .get("stdout")
            .and_then(Value::as_str)
            .map(str::to_owned),
        "postgres.explain" => data
            .get("plan")
            .and_then(|plan| serde_json::to_string_pretty(plan).ok()),
        _ => None,
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

fn tool_status_descriptor() -> CommandDescriptor {
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

fn tool_download_descriptor() -> CommandDescriptor {
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

fn tool_use_descriptor() -> CommandDescriptor {
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

fn tool_cleanup_descriptor() -> CommandDescriptor {
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

fn ping_descriptor() -> CommandDescriptor {
    database_descriptor::<NoArgs>(
        "postgres.ping",
        "Ping PostgreSQL",
        "Connect and return server plus session identity metadata.",
        output_schema_for::<InfoOutput>("postgres.ping"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

fn info_descriptor() -> CommandDescriptor {
    database_descriptor::<NoArgs>(
        "postgres.info",
        "Inspect PostgreSQL session",
        "Return selected server and session metadata.",
        output_schema_for::<InfoOutput>("postgres.info"),
        "Reads server/session metadata. ensure_tool=true may first download and write a shared client toolchain.",
    )
}

fn databases_descriptor() -> CommandDescriptor {
    rows_descriptor::<NoArgs, DatabaseRow>(
        "postgres.databases",
        "List PostgreSQL databases",
        "Reads database names, owners, encodings, connection flags, and visible size information.",
    )
}

fn schemas_descriptor() -> CommandDescriptor {
    rows_descriptor::<IncludeSystemArgs, SchemaRow>(
        "postgres.schemas",
        "List PostgreSQL schemas",
        "Reads schema names and owners, optionally including system schemas.",
    )
}

fn relations_descriptor(id: &str, title: &str) -> CommandDescriptor {
    rows_descriptor::<RelationListArgs, RelationRow>(
        id,
        title,
        "Reads relation names, owners, row estimates, and total sizes.",
    )
}

fn describe_descriptor() -> CommandDescriptor {
    database_descriptor::<DescribeArgs>(
        "postgres.describe",
        "Describe PostgreSQL relation",
        "Describe a table, view, or materialized view.",
        output_schema_for::<DescribeOutput>("postgres.describe"),
        "Reads relation, column, index, and constraint definitions that may reveal database structure.",
    )
}

fn indexes_descriptor() -> CommandDescriptor {
    rows_descriptor::<IndexesArgs, IndexRow>(
        "postgres.indexes",
        "List PostgreSQL indexes",
        "Reads index names and full definitions.",
    )
}

fn extensions_descriptor() -> CommandDescriptor {
    rows_descriptor::<ExtensionsArgs, ExtensionRow>(
        "postgres.extensions",
        "List PostgreSQL extensions",
        "Reads installed extension metadata or the server's available extension catalog.",
    )
}

fn query_descriptor() -> CommandDescriptor {
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

fn exec_descriptor() -> CommandDescriptor {
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

fn explain_descriptor() -> CommandDescriptor {
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

fn activity_descriptor() -> CommandDescriptor {
    rows_descriptor::<ActivityArgs, ActivityRow>(
        "postgres.activity",
        "Inspect PostgreSQL activity",
        "Reads session identities, client addresses, wait states, timestamps, and truncated SQL text from pg_stat_activity.",
    )
}

fn locks_descriptor() -> CommandDescriptor {
    rows_descriptor::<LocksArgs, LockRow>(
        "postgres.locks",
        "Inspect PostgreSQL locks",
        "Reads blocked and blocking session identities plus truncated SQL text.",
    )
}

fn size_descriptor() -> CommandDescriptor {
    rows_descriptor::<SizeArgs, SizeRow>(
        "postgres.size",
        "Inspect PostgreSQL sizes",
        "Reads database, schema, or relation size statistics.",
    )
}

fn settings_descriptor() -> CommandDescriptor {
    rows_descriptor::<SettingsArgs, SettingRow>(
        "postgres.settings",
        "Inspect PostgreSQL settings",
        "Reads server settings, sources, units, and descriptions; configuration values may contain sensitive operational details.",
    )
}

fn rows_descriptor<A: JsonSchema, T: JsonSchema>(
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

fn database_descriptor<A: JsonSchema>(
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

fn postgres_secret_slot() -> SecretSlot {
    SecretSlot::optional("database", ["postgres"], "PostgreSQL database credential.")
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

#[cfg(test)]
mod tests {
    use ah_plugin_api::BindResolvedSecrets;
    use std::collections::BTreeMap;

    use ah_plugin_api::{ExecutionContextWire, ResolvedSecret};

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
    fn direct_cli_binds_a_resolved_password_without_exposing_it() {
        use ah_plugin_api::{BindResolvedSecrets, ResolvedSecret};

        let mut cli = parse_args(&[
            "query".to_owned(),
            "--database".to_owned(),
            "app".to_owned(),
            "--sql".to_owned(),
            "select 1".to_owned(),
        ])
        .expect("legacy argv should parse");

        cli.bind_resolved_secrets(&std::collections::BTreeMap::from([(
            "database".to_owned(),
            ResolvedSecret {
                id: "app-db".to_owned(),
                kind: "postgres".to_owned(),
                values: std::collections::BTreeMap::from([(
                    "password".to_owned(),
                    "direct-cli-sentinel".to_owned(),
                )]),
            },
        )]))
        .expect("resolved credential should bind");

        let password = cli
            .connection
            .resolved_password
            .as_ref()
            .expect("password should be bound");
        assert_eq!(password.expose(), "direct-cli-sentinel");
        assert!(!format!("{:?}", cli.connection).contains("direct-cli-sentinel"));
    }

    #[test]
    fn direct_tool_command_rejects_a_database_credential() {
        let mut cli = super::super::PostgresCli::try_parse_from(["postgres", "tool", "status"])
            .expect("tool status should parse");

        let error = cli
            .bind_resolved_secrets(&std::collections::BTreeMap::from([(
                "database".to_owned(),
                ResolvedSecret {
                    id: "app-db".to_owned(),
                    kind: "postgres".to_owned(),
                    values: std::collections::BTreeMap::from([(
                        "password".to_owned(),
                        "private-password".to_owned(),
                    )]),
                },
            )]))
            .expect_err("tool commands must not accept database credentials");

        assert_eq!(
            error.error_code.expect("structured error code"),
            "INVALID_ARGUMENT"
        );
    }

    #[test]
    fn exec_requires_explicit_confirmation_in_schema() {
        let descriptor = exec_descriptor();
        assert_eq!(descriptor.input_schema["properties"]["yes"]["const"], true);
        assert!(descriptor.effects.destructive);
        assert_eq!(descriptor.effects.risk, RiskLevel::Critical);
    }

    #[test]
    fn connection_schema_explains_password_environment_usage() {
        let descriptor = query_descriptor();
        assert!(
            descriptor.input_schema["properties"]["password_env"]["description"]
                .as_str()
                .unwrap()
                .contains("password-protected")
        );
    }

    #[test]
    fn database_commands_declare_only_the_postgres_secret_slot() {
        let catalog = command_catalog();

        for command in &catalog.commands {
            if command.id.starts_with("postgres.tool.") {
                assert!(command.secret_slots.is_empty(), "{}", command.id);
            } else {
                assert_eq!(command.secret_slots.len(), 1, "{}", command.id);
                let slot = &command.secret_slots[0];
                assert_eq!(slot.name, "database");
                assert_eq!(slot.accepted_kinds, ["postgres"]);
                assert!(!slot.required);
            }
        }
    }

    #[test]
    fn typed_connection_uses_matching_resolved_database_password() {
        let request = TypedInvocationRequest::new(
            "postgres.ping",
            json!({"credentials": {"database": "qa-db"}}),
            ExecutionContextWire::new("postgres-secret", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "database".to_owned(),
            ResolvedSecret {
                id: "qa-db".to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([(
                    "password".to_owned(),
                    "postgres-private-sentinel".to_owned(),
                )]),
            },
        )]));

        let cli = typed_cli(&request).expect("resolved credential should bind");

        assert_eq!(
            cli.connection
                .resolved_password
                .as_ref()
                .map(SecretValue::expose),
            Some("postgres-private-sentinel")
        );
    }

    #[test]
    fn typed_connection_rejects_password_env_with_database_credential() {
        let request = TypedInvocationRequest::new(
            "postgres.ping",
            json!({
                "password_env": "LEGACY_DATABASE_PASSWORD",
                "credentials": {"database": "qa-db"}
            }),
            ExecutionContextWire::new("postgres-secret-conflict", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "database".to_owned(),
            ResolvedSecret {
                id: "qa-db".to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([(
                    "password".to_owned(),
                    "postgres-conflict-sentinel".to_owned(),
                )]),
            },
        )]));

        let error = typed_cli(&request).expect_err("password sources must conflict");
        let serialized = serde_json::to_string(&error).unwrap();
        assert_eq!(error.code, "INVALID_ARGUMENT");
        assert!(!serialized.contains("postgres-conflict-sentinel"));
    }

    #[test]
    fn typed_connection_rejects_unresolved_or_wrong_database_slots_without_leaking_values() {
        let unresolved = TypedInvocationRequest::new(
            "postgres.ping",
            json!({"credentials": {"database": "qa-db"}}),
            ExecutionContextWire::new("postgres-unresolved", ".", None, 1_000),
        );
        let error = typed_cli(&unresolved).expect_err("public id requires private resolution");
        assert_eq!(error.code, "SECRET_REQUIRED");

        let wrong_slot = TypedInvocationRequest::new(
            "postgres.ping",
            json!({"credentials": {"database": "qa-db"}}),
            ExecutionContextWire::new("postgres-wrong-slot", ".", None, 1_000),
        )
        .with_resolved_secrets(BTreeMap::from([(
            "other".to_owned(),
            ResolvedSecret {
                id: "qa-db".to_owned(),
                kind: "postgres".to_owned(),
                values: BTreeMap::from([(
                    "password".to_owned(),
                    "postgres-leak-sentinel".to_owned(),
                )]),
            },
        )]));
        let response = TypedInvocationResponse::error(
            typed_cli(&wrong_slot).expect_err("unsupported private slot must fail"),
        );
        let serialized = serde_json::to_string(&response).expect("response serializes");
        assert!(!serialized.contains("postgres-leak-sentinel"));
    }
}
