use std::{
    env,
    ffi::c_char,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
    thread,
    time::{Duration, Instant},
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AhPluginApiV1, GlobalOptionsWire, InvocationRequest, InvocationResponse,
    ManualCommand, ManualExample, PluginManual, c_ptr_to_string, free_c_string_ptr,
    manual_to_c_string, response_to_c_string,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::blocking::Client;
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
const AH_POSTGRES_TEST_SYSTEM_PATH: &str = "AH_POSTGRES_TEST_SYSTEM_PATH";
const POSTGRES_18_4_WINDOWS_X64_URL: &str =
    "https://get.enterprisedb.com/postgresql/postgresql-18.4-1-windows-x64-binaries.zip";
const POSTGRES_18_4_WINDOWS_X64_SHA256: &str =
    "7effe34c0bf89027b3f171447d351cbc460f4566c8d0f643daec67f140787858";

static PLUGIN_NAME_C: &[u8] = b"external-postgres\0";
static DOMAIN_C: &[u8] = b"postgres\0";
static DESCRIPTION_C: &[u8] = b"PostgreSQL database workflow plugin (dynamic)\0";

static PLUGIN_API_PTR: AtomicPtr<AhPluginApiV1> = AtomicPtr::new(ptr::null_mut());

#[derive(Debug, Parser)]
#[command(name = "postgres", about = "PostgreSQL database workflow helpers")]
struct PostgresCli {
    #[command(flatten)]
    tool: ToolResolverArgs,
    #[command(flatten)]
    connection: ConnectionArgs,
    #[command(subcommand)]
    command: PostgresCommand,
}

#[derive(Debug, Args, Clone)]
struct ToolResolverArgs {
    #[arg(long, global = true, value_name = "PATH")]
    tool_path: Option<PathBuf>,
    #[arg(long, global = true)]
    ensure_tool: bool,
}

#[derive(Debug, Args, Clone)]
struct ConnectionArgs {
    #[arg(long, global = true, value_name = "HOST")]
    host: Option<String>,
    #[arg(long, global = true, value_name = "PORT")]
    port: Option<u16>,
    #[arg(long, global = true, value_name = "NAME")]
    database: Option<String>,
    #[arg(long, global = true, value_name = "USER")]
    user: Option<String>,
    #[arg(long, global = true, value_name = "NAME")]
    service: Option<String>,
    #[arg(
        long,
        global = true,
        value_name = "MODE",
        value_parser = ["disable", "allow", "prefer", "require", "verify-ca", "verify-full"]
    )]
    sslmode: Option<String>,
    #[arg(long, global = true, value_name = "ENV_VAR")]
    password_env: Option<String>,
    #[arg(long, global = true, default_value_t = DEFAULT_CONNECT_TIMEOUT_SECS, value_name = "SECONDS")]
    connect_timeout_secs: u64,
    #[arg(long, global = true, value_name = "MILLISECONDS")]
    statement_timeout_ms: Option<u64>,
}

#[derive(Debug, Subcommand)]
enum PostgresCommand {
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
struct ToolArgs {
    #[command(subcommand)]
    command: ToolCommand,
}

#[derive(Debug, Subcommand)]
enum ToolCommand {
    #[command(about = "Show resolved PostgreSQL toolchain status")]
    Status,
    #[command(about = "Download a managed PostgreSQL toolchain")]
    Download(ToolDownloadArgs),
    #[command(about = "Persist an explicit PostgreSQL toolchain path")]
    Use(ToolUseArgs),
    #[command(about = "Remove managed PostgreSQL toolchain cache")]
    Cleanup(ToolCleanupArgs),
}

#[derive(Debug, Args)]
struct ToolDownloadArgs {
    #[arg(long, default_value = DEFAULT_POSTGRES_VERSION, value_name = "VERSION")]
    version: String,
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct ToolUseArgs {
    #[arg(long, value_name = "PATH")]
    path: PathBuf,
}

#[derive(Debug, Args)]
struct ToolCleanupArgs {
    #[arg(long, value_name = "VERSION")]
    version: Option<String>,
}

#[derive(Debug, Args)]
struct IncludeSystemArgs {
    #[arg(long)]
    include_system: bool,
}

#[derive(Debug, Args)]
struct RelationListArgs {
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    #[arg(long)]
    include_system: bool,
}

#[derive(Debug, Args)]
struct DescribeArgs {
    object: String,
}

#[derive(Debug, Args)]
struct IndexesArgs {
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    #[arg(long, value_name = "NAME")]
    table: Option<String>,
}

#[derive(Debug, Args)]
struct ExtensionsArgs {
    #[arg(long)]
    available: bool,
}

#[derive(Debug, Args)]
struct QueryArgs {
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    sql: Option<String>,
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ExecArgs {
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    sql: Option<String>,
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    #[arg(long)]
    single_transaction: bool,
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct ExplainArgs {
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    sql: Option<String>,
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    #[arg(long)]
    analyze: bool,
    #[arg(long)]
    buffers: bool,
    #[arg(long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct ActivityArgs {
    #[arg(long)]
    active: bool,
    #[arg(long)]
    idle_in_tx: bool,
}

#[derive(Debug, Args)]
struct LocksArgs {
    #[arg(long)]
    blocking: bool,
}

#[derive(Debug, Args)]
struct SizeArgs {
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    #[arg(long, value_name = "NAME")]
    table: Option<String>,
}

#[derive(Debug, Args)]
struct SettingsArgs {
    #[arg(long)]
    changed: bool,
}

#[derive(Debug, Clone)]
struct ToolContext {
    psql_path: PathBuf,
    bin_dir: PathBuf,
    version: ToolVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ToolSource {
    Explicit,
    Env,
    Configured,
    ManagedCache,
    SystemPath,
}

impl ToolSource {
    fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Env => "env",
            Self::Configured => "configured",
            Self::ManagedCache => "managed-cache",
            Self::SystemPath => "system-path",
        }
    }

    fn is_explicit_intent(self) -> bool {
        matches!(self, Self::Explicit | Self::Env | Self::Configured)
    }
}

#[derive(Debug, Clone, Serialize)]
struct ToolVersion {
    raw: String,
    major: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct CandidateStatus {
    source: &'static str,
    path: PathBuf,
    psql_path: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    version_raw: Option<String>,
    version_major: Option<u32>,
    accepted: bool,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ToolStatusOutput {
    command: &'static str,
    available: bool,
    selected: Option<CandidateStatus>,
    candidates: Vec<CandidateStatus>,
    target_version: &'static str,
    minimum_major: u32,
    cache_dir: Option<PathBuf>,
    config_path: Option<PathBuf>,
    remediation: Option<String>,
}

#[derive(Debug, Serialize)]
struct ToolDownloadOutput {
    command: &'static str,
    version: String,
    url: String,
    sha256: String,
    cache_path: PathBuf,
    bin_dir: PathBuf,
    psql_path: PathBuf,
    downloaded: bool,
}

#[derive(Debug, Serialize)]
struct ToolUseOutput {
    command: &'static str,
    path: PathBuf,
    psql_path: PathBuf,
    version: ToolVersion,
    config_path: PathBuf,
}

#[derive(Debug, Serialize)]
struct ToolCleanupOutput {
    command: &'static str,
    removed: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct MissingToolOutput {
    available: bool,
    required: String,
    target: &'static str,
    searched: Vec<CandidateStatus>,
    remediation: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolConfig {
    version: u32,
    path: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct InfoRow {
    server_version: String,
    current_database: String,
    current_user: String,
    session_user: String,
    current_schema: Option<String>,
    server_encoding: String,
    inet_server_addr: Option<String>,
    inet_server_port: Option<i32>,
}

#[derive(Debug, Serialize)]
struct InfoOutput {
    command: &'static str,
    #[serde(flatten)]
    info: InfoRow,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct DatabaseRow {
    name: String,
    owner: String,
    encoding: String,
    allow_connections: bool,
    size: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SchemaRow {
    name: String,
    owner: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct RelationRow {
    schema: String,
    name: String,
    kind: String,
    owner: String,
    rows_estimate: Option<i64>,
    size: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ColumnRow {
    ordinal: i32,
    name: String,
    data_type: String,
    nullable: bool,
    default: Option<String>,
    comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct IndexRow {
    schema: String,
    table: String,
    name: String,
    primary: bool,
    unique: bool,
    definition: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ConstraintRow {
    name: String,
    constraint_type: String,
    definition: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct DescribeRelationRow {
    schema: String,
    name: String,
    kind: String,
    owner: String,
    rows_estimate: Option<i64>,
    total_size: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct DescribeOutput {
    command: &'static str,
    relation: DescribeRelationRow,
    columns: Vec<ColumnRow>,
    indexes: Vec<IndexRow>,
    constraints: Vec<ConstraintRow>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ExtensionRow {
    name: String,
    installed_version: Option<String>,
    default_version: Option<String>,
    schema: Option<String>,
    comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActivityRow {
    pid: i32,
    user: Option<String>,
    database: Option<String>,
    application_name: Option<String>,
    client_addr: Option<String>,
    state: Option<String>,
    wait_event_type: Option<String>,
    wait_event: Option<String>,
    query_start: Option<String>,
    state_change: Option<String>,
    query: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct LockRow {
    blocked_pid: i32,
    blocked_user: Option<String>,
    blocking_pid: Option<i32>,
    blocking_user: Option<String>,
    lock_type: String,
    mode: String,
    relation: Option<String>,
    blocked_query: Option<String>,
    blocking_query: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SizeRow {
    scope: String,
    schema: Option<String>,
    name: String,
    size: String,
    bytes: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SettingRow {
    name: String,
    setting: String,
    unit: Option<String>,
    source: String,
    short_desc: String,
}

#[derive(Debug, Serialize)]
struct RowsOutput<T> {
    command: &'static str,
    count: usize,
    rows: Vec<T>,
}

#[derive(Debug, Serialize)]
struct QueryOutput {
    command: &'static str,
    row_count: usize,
    rows: Value,
}

#[derive(Debug, Serialize)]
struct ExecOutput {
    command: &'static str,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Serialize)]
struct ExplainOutput {
    command: &'static str,
    analyze: bool,
    buffers: bool,
    plan: Value,
}

/// Returns the PostgreSQL plugin ABI entry point.
///
/// # Safety
///
/// The returned pointer is process-static and must not be freed or mutated by
/// the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_entry_v1() -> *const AhPluginApiV1 {
    let existing = PLUGIN_API_PTR.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing.cast_const();
    }

    let created = Box::into_raw(Box::new(AhPluginApiV1 {
        abi_version: AH_PLUGIN_ABI_VERSION,
        plugin_name: PLUGIN_NAME_C.as_ptr().cast(),
        domain: DOMAIN_C.as_ptr().cast(),
        description: DESCRIPTION_C.as_ptr().cast(),
        invoke_json: ah_plugin_invoke_json,
        free_c_string: ah_plugin_free_c_string,
    }));

    match PLUGIN_API_PTR.compare_exchange(
        ptr::null_mut(),
        created,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => created.cast_const(),
        Err(existing) => {
            unsafe { drop(Box::from_raw(created)) };
            existing.cast_const()
        }
    }
}

/// Returns the PostgreSQL plugin manual JSON as an owned C string.
///
/// # Safety
///
/// The caller must free the returned pointer with this plugin's
/// `free_c_string` callback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_manual_json_v1() -> *mut c_char {
    manual_to_c_string(&plugin_manual())
}

unsafe extern "C" fn ah_plugin_invoke_json(request_json: *const c_char) -> *mut c_char {
    let response = invoke_from_raw(request_json);
    response_to_c_string(&response)
}

unsafe extern "C" fn ah_plugin_free_c_string(value: *mut c_char) {
    unsafe { free_c_string_ptr(value) };
}

fn invoke_from_raw(request_json: *const c_char) -> InvocationResponse {
    let request_json = match unsafe { c_ptr_to_string(request_json) } {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request pointer: {error}"),
            );
        }
    };

    let request = match serde_json::from_str::<InvocationRequest>(&request_json) {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request JSON: {error}"),
            );
        }
    };

    if request.domain != DOMAIN {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!(
                "plugin domain mismatch: expected '{DOMAIN}', got '{}'",
                request.domain
            ),
        );
    }

    let parsed = match parse_args(&request.argv) {
        Ok(value) => value,
        Err(response) => return response,
    };

    execute(parsed, &request.globals)
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

fn execute_tool(
    args: ToolArgs,
    resolver_args: &ToolResolverArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        ToolCommand::Status => execute_tool_status(resolver_args, globals),
        ToolCommand::Download(args) => execute_tool_download(args, globals),
        ToolCommand::Use(args) => execute_tool_use(args, globals),
        ToolCommand::Cleanup(args) => execute_tool_cleanup(args, globals),
    }
}

fn execute_tool_status(
    resolver_args: &ToolResolverArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let candidates = inspect_tool_candidates(resolver_args);
    let selected = candidates
        .iter()
        .find(|candidate| candidate.accepted)
        .cloned();
    let output = ToolStatusOutput {
        command: "postgres.tool.status",
        available: selected.is_some(),
        selected,
        candidates,
        target_version: DEFAULT_POSTGRES_VERSION,
        minimum_major: MIN_POSTGRES_MAJOR,
        cache_dir: postgres_cache_root().ok(),
        config_path: tool_config_path().ok(),
        remediation: Some("ah postgres tool download".to_owned()),
    };

    render_success(globals, &output, render_tool_status_text(&output))
}

fn execute_tool_download(
    args: ToolDownloadArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match download_managed_tool(&args.version, args.force) {
        Ok(output) => render_success(globals, &output, render_tool_download_text(&output)),
        Err(error) => error,
    }
}

fn execute_tool_use(args: ToolUseArgs, globals: &GlobalOptionsWire) -> InvocationResponse {
    let candidate = match evaluate_candidate(ToolSource::Configured, args.path.clone()) {
        CandidateEvaluation::Accepted(context) => context,
        CandidateEvaluation::Rejected(candidate) => {
            return InvocationResponse::error(
                "POSTGRES_TOOL_UNAVAILABLE",
                format!(
                    "configured PostgreSQL tool path '{}' is not usable: {}",
                    args.path.display(),
                    candidate
                        .reason
                        .unwrap_or_else(|| "unknown validation failure".to_owned())
                ),
            );
        }
    };

    let config_path = match tool_config_path() {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Err(error) = write_tool_config(&ToolConfig {
        version: SETTINGS_VERSION,
        path: Some(candidate.bin_dir.clone()),
    }) {
        return error;
    }

    let output = ToolUseOutput {
        command: "postgres.tool.use",
        path: candidate.bin_dir.clone(),
        psql_path: candidate.psql_path.clone(),
        version: candidate.version.clone(),
        config_path,
    };
    render_success(
        globals,
        &output,
        format!("using PostgreSQL toolchain at {}\n", output.path.display()),
    )
}

fn execute_tool_cleanup(args: ToolCleanupArgs, globals: &GlobalOptionsWire) -> InvocationResponse {
    let cache_root = match postgres_cache_root() {
        Ok(path) => path,
        Err(error) => return error,
    };
    let mut removed = Vec::new();
    if let Some(version) = args.version {
        let path = cache_root.join(version);
        if path.exists() {
            if let Err(error) = remove_cache_dir(&cache_root, &path) {
                return error;
            }
            removed.push(path);
        }
    } else if cache_root.exists() {
        let entries = match fs::read_dir(&cache_root) {
            Ok(entries) => entries,
            Err(error) => {
                return InvocationResponse::error(
                    "POSTGRES_TOOL_CLEANUP_FAILED",
                    format!(
                        "failed to read PostgreSQL tool cache '{}': {error}",
                        cache_root.display()
                    ),
                );
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) => {
                    return InvocationResponse::error(
                        "POSTGRES_TOOL_CLEANUP_FAILED",
                        format!("failed to read PostgreSQL tool cache entry: {error}"),
                    );
                }
            };
            let path = entry.path();
            if path.is_dir() {
                if let Err(error) = remove_cache_dir(&cache_root, &path) {
                    return error;
                }
                removed.push(path);
            }
        }
    }

    let output = ToolCleanupOutput {
        command: "postgres.tool.cleanup",
        removed,
    };
    let text = if output.removed.is_empty() {
        "no managed PostgreSQL tool cache entries removed\n".to_owned()
    } else {
        format!(
            "removed {} managed PostgreSQL tool cache entr{}\n",
            output.removed.len(),
            if output.removed.len() == 1 {
                "y"
            } else {
                "ies"
            }
        )
    };
    render_success(globals, &output, text)
}

fn resolve_operational_tool(args: &ToolResolverArgs) -> Result<ToolContext, InvocationResponse> {
    let candidates = inspect_tool_candidates(args);
    if let Some(first) = candidates.first()
        && source_from_label(first.source)
            .map(ToolSource::is_explicit_intent)
            .unwrap_or(false)
        && !first.accepted
    {
        return Err(missing_tool_response(candidates));
    }

    if let Some(candidate) = candidates.iter().find(|candidate| candidate.accepted) {
        return accepted_candidate_to_context(candidate)
            .ok_or_else(|| missing_tool_response(candidates));
    }

    if args.ensure_tool {
        download_managed_tool(DEFAULT_POSTGRES_VERSION, false)?;
        let candidates = inspect_tool_candidates(args);
        if let Some(candidate) = candidates.iter().find(|candidate| candidate.accepted) {
            return accepted_candidate_to_context(candidate)
                .ok_or_else(|| missing_tool_response(candidates));
        }
        return Err(missing_tool_response(candidates));
    }

    Err(missing_tool_response(candidates))
}

fn missing_tool_response(candidates: Vec<CandidateStatus>) -> InvocationResponse {
    let output = MissingToolOutput {
        available: false,
        required: format!("psql >= {MIN_POSTGRES_MAJOR}"),
        target: DEFAULT_POSTGRES_VERSION,
        searched: candidates,
        remediation: vec![
            "ah postgres tool download".to_owned(),
            "ah postgres tool use --path PATH".to_owned(),
        ],
    };
    let details = serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_owned());
    InvocationResponse::error(
        "POSTGRES_TOOL_UNAVAILABLE",
        format!(
            "PostgreSQL toolchain is not available. Run: ah postgres tool download. Or set a path: ah postgres tool use --path PATH. Details: {details}"
        ),
    )
}

enum CandidateEvaluation {
    Accepted(ToolContext),
    Rejected(CandidateStatus),
}

fn inspect_tool_candidates(args: &ToolResolverArgs) -> Vec<CandidateStatus> {
    let mut candidates = Vec::new();
    if let Some(path) = &args.tool_path {
        candidates.push(candidate_status(ToolSource::Explicit, path.clone()));
        return candidates;
    }
    if let Some(path) = env::var_os(AH_POSTGRES_TOOL_PATH)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        candidates.push(candidate_status(ToolSource::Env, path));
        return candidates;
    }
    if let Ok(config) = read_tool_config()
        && let Some(path) = config.path
    {
        candidates.push(candidate_status(ToolSource::Configured, path));
        return candidates;
    }
    if let Ok(path) = managed_tool_path(DEFAULT_POSTGRES_VERSION)
        && path.exists()
    {
        candidates.push(candidate_status(ToolSource::ManagedCache, path));
    }
    if let Some(path) = find_psql_in_path() {
        candidates.push(candidate_status(ToolSource::SystemPath, path));
    }
    candidates
}

fn candidate_status(source: ToolSource, path: PathBuf) -> CandidateStatus {
    match evaluate_candidate(source, path.clone()) {
        CandidateEvaluation::Accepted(context) => CandidateStatus {
            source: source.label(),
            path,
            psql_path: Some(context.psql_path),
            bin_dir: Some(context.bin_dir),
            version_raw: Some(context.version.raw),
            version_major: context.version.major,
            accepted: true,
            reason: None,
        },
        CandidateEvaluation::Rejected(status) => status,
    }
}

fn evaluate_candidate(source: ToolSource, path: PathBuf) -> CandidateEvaluation {
    let (psql_path, bin_dir) = match resolve_psql_path(&path) {
        Ok(value) => value,
        Err(reason) => {
            return CandidateEvaluation::Rejected(CandidateStatus {
                source: source.label(),
                path,
                psql_path: None,
                bin_dir: None,
                version_raw: None,
                version_major: None,
                accepted: false,
                reason: Some(reason),
            });
        }
    };

    match psql_version(&psql_path, &bin_dir) {
        Ok(version) => {
            let accepted = version.major.unwrap_or(0) >= MIN_POSTGRES_MAJOR;
            if accepted {
                CandidateEvaluation::Accepted(ToolContext {
                    psql_path,
                    bin_dir,
                    version,
                })
            } else {
                CandidateEvaluation::Rejected(CandidateStatus {
                    source: source.label(),
                    path,
                    psql_path: Some(psql_path),
                    bin_dir: Some(bin_dir),
                    version_raw: Some(version.raw),
                    version_major: version.major,
                    accepted: false,
                    reason: Some(format!("psql major version is below {MIN_POSTGRES_MAJOR}")),
                })
            }
        }
        Err(reason) => CandidateEvaluation::Rejected(CandidateStatus {
            source: source.label(),
            path,
            psql_path: Some(psql_path),
            bin_dir: Some(bin_dir),
            version_raw: None,
            version_major: None,
            accepted: false,
            reason: Some(reason),
        }),
    }
}

fn accepted_candidate_to_context(candidate: &CandidateStatus) -> Option<ToolContext> {
    if !candidate.accepted {
        return None;
    }
    let psql_path = candidate.psql_path.clone()?;
    let bin_dir = candidate.bin_dir.clone()?;
    let version = ToolVersion {
        raw: candidate.version_raw.clone()?,
        major: candidate.version_major,
    };
    Some(ToolContext {
        psql_path,
        bin_dir,
        version,
    })
}

fn source_from_label(label: &str) -> Option<ToolSource> {
    match label {
        "explicit" => Some(ToolSource::Explicit),
        "env" => Some(ToolSource::Env),
        "configured" => Some(ToolSource::Configured),
        "managed-cache" => Some(ToolSource::ManagedCache),
        "system-path" => Some(ToolSource::SystemPath),
        _ => None,
    }
}

fn resolve_psql_path(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    if path.is_file() {
        let bin_dir = path
            .parent()
            .ok_or_else(|| {
                format!(
                    "tool executable '{}' has no parent directory",
                    path.display()
                )
            })?
            .to_path_buf();
        return Ok((path.to_path_buf(), bin_dir));
    }
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!(
            "path is not a file or directory: {}",
            path.display()
        ));
    }

    let candidates = [
        path.join(psql_exe_name()),
        path.join("pgsql").join("bin").join(psql_exe_name()),
        path.join("bin").join(psql_exe_name()),
    ];
    for psql_path in candidates {
        if psql_path.is_file() {
            let bin_dir = psql_path
                .parent()
                .ok_or_else(|| {
                    format!(
                        "tool executable '{}' has no parent directory",
                        psql_path.display()
                    )
                })?
                .to_path_buf();
            return Ok((psql_path, bin_dir));
        }
    }
    Err(format!(
        "no {} found under '{}'",
        psql_exe_name(),
        path.display()
    ))
}

fn psql_version(psql_path: &Path, bin_dir: &Path) -> Result<ToolVersion, String> {
    let output = Command::new(psql_path)
        .arg("--version")
        .env("PATH", prepend_path_env(bin_dir))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "failed to execute '{} --version': {error}",
                psql_path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "'{} --version' failed with exit code {:?}: {}",
            psql_path.display(),
            output.status.code(),
            truncate_for_error(&String::from_utf8_lossy(&output.stderr), 400)
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(ToolVersion {
        major: parse_psql_major(&raw),
        raw,
    })
}

fn parse_psql_major(raw: &str) -> Option<u32> {
    raw.split_whitespace().find_map(|part| {
        let first = part.split('.').next()?;
        first.parse::<u32>().ok()
    })
}

fn find_psql_in_path() -> Option<PathBuf> {
    if let Some(test_path) = env::var_os(AH_POSTGRES_TEST_SYSTEM_PATH) {
        return if test_path.is_empty() {
            None
        } else {
            Some(PathBuf::from(test_path))
        };
    }
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        for name in path_executable_names("psql") {
            let candidate = dir.join(&name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn path_executable_names(base: &str) -> Vec<String> {
    if cfg!(windows) {
        let mut names = Vec::new();
        names.push(format!("{base}.exe"));
        if let Some(pathext) = env::var_os("PATHEXT") {
            for ext in env::split_paths(&pathext) {
                if let Some(ext) = ext.to_str() {
                    names.push(format!("{base}{ext}"));
                }
            }
        }
        names.sort();
        names.dedup();
        names
    } else {
        vec![base.to_owned()]
    }
}

fn psql_exe_name() -> &'static str {
    if cfg!(windows) { "psql.exe" } else { "psql" }
}

fn download_managed_tool(
    version: &str,
    force: bool,
) -> Result<ToolDownloadOutput, InvocationResponse> {
    let manifest = match download_manifest(version) {
        Some(value) => value,
        None => {
            return Err(InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_UNSUPPORTED",
                format!(
                    "managed PostgreSQL tool download is not available for version '{}' on this platform",
                    version
                ),
            ));
        }
    };
    let cache_root = postgres_cache_root()?;
    let version_dir = cache_root.join(version);
    let bin_dir = version_dir.join("pgsql").join("bin");
    let psql_path = bin_dir.join(psql_exe_name());
    fs::create_dir_all(&cache_root).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "failed to create PostgreSQL tool cache '{}': {error}",
                cache_root.display()
            ),
        )
    })?;
    let _download_lock = acquire_download_lock(&cache_root, version)?;

    if psql_path.exists()
        && !force
        && let CandidateEvaluation::Accepted(_) =
            evaluate_candidate(ToolSource::ManagedCache, version_dir.clone())
    {
        return Ok(ToolDownloadOutput {
            command: "postgres.tool.download",
            version: version.to_owned(),
            url: manifest.url.to_owned(),
            sha256: manifest.sha256.to_owned(),
            cache_path: version_dir,
            bin_dir,
            psql_path,
            downloaded: false,
        });
    }

    let archive_path = cache_root.join(format!(
        "postgresql-{}-{}.zip.download",
        version, manifest.platform
    ));
    download_archive(manifest.url, manifest.sha256, &archive_path)?;

    let temp_dir = cache_root.join(format!("{}.tmp.{}", version, std::process::id()));
    if temp_dir.exists() {
        remove_cache_dir(&cache_root, &temp_dir)?;
    }
    fs::create_dir_all(&temp_dir).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to create temporary extract directory '{}': {error}",
                temp_dir.display()
            ),
        )
    })?;
    extract_archive(&archive_path, &temp_dir)?;
    if !temp_dir
        .join("pgsql")
        .join("bin")
        .join(psql_exe_name())
        .is_file()
    {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "extracted archive did not contain pgsql/bin/{}",
                psql_exe_name()
            ),
        ));
    }
    if version_dir.exists() {
        remove_cache_dir(&cache_root, &version_dir)?;
    }
    fs::rename(&temp_dir, &version_dir).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to move extracted PostgreSQL tools into '{}': {error}",
                version_dir.display()
            ),
        )
    })?;
    let _ = fs::remove_file(&archive_path);

    Ok(ToolDownloadOutput {
        command: "postgres.tool.download",
        version: version.to_owned(),
        url: manifest.url.to_owned(),
        sha256: manifest.sha256.to_owned(),
        cache_path: version_dir,
        bin_dir,
        psql_path,
        downloaded: true,
    })
}

struct DownloadLock {
    path: PathBuf,
}

impl Drop for DownloadLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_download_lock(
    cache_root: &Path,
    version: &str,
) -> Result<DownloadLock, InvocationResponse> {
    let lock_path = cache_root.join(format!("{version}.lock"));
    let start = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={}", std::process::id());
                return Ok(DownloadLock { path: lock_path });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&lock_path) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if start.elapsed() >= Duration::from_secs(120) {
                    return Err(InvocationResponse::error(
                        "POSTGRES_TOOL_DOWNLOAD_FAILED",
                        format!(
                            "timed out waiting for PostgreSQL tool download lock '{}'",
                            lock_path.display()
                        ),
                    ));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(error) => {
                return Err(InvocationResponse::error(
                    "POSTGRES_TOOL_DOWNLOAD_FAILED",
                    format!(
                        "failed to create PostgreSQL tool download lock '{}': {error}",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
}

fn lock_is_stale(lock_path: &Path) -> bool {
    fs::metadata(lock_path)
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| {
            modified
                .elapsed()
                .map_err(|error| io::Error::other(error.to_string()))
        })
        .map(|age| age > Duration::from_secs(30 * 60))
        .unwrap_or(false)
}

struct DownloadManifest {
    url: &'static str,
    sha256: &'static str,
    platform: &'static str,
}

fn download_manifest(version: &str) -> Option<DownloadManifest> {
    if version == DEFAULT_POSTGRES_VERSION
        && cfg!(target_os = "windows")
        && cfg!(target_arch = "x86_64")
    {
        return Some(DownloadManifest {
            url: POSTGRES_18_4_WINDOWS_X64_URL,
            sha256: POSTGRES_18_4_WINDOWS_X64_SHA256,
            platform: "windows-x64",
        });
    }
    None
}

fn download_archive(
    url: &str,
    expected_sha256: &str,
    archive_path: &Path,
) -> Result<(), InvocationResponse> {
    let client = Client::builder()
        .timeout(Duration::from_secs(DEFAULT_DOWNLOAD_TIMEOUT_SECS))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;
    let mut response = client.get(url).send().map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!("failed to download PostgreSQL tool archive from '{url}': {error}"),
        )
    })?;
    if !response.status().is_success() {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "PostgreSQL tool archive download returned HTTP {} from '{}'",
                response.status(),
                url
            ),
        ));
    }
    let mut file = File::create(archive_path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "failed to create download file '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response.read(&mut buffer).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed while reading PostgreSQL archive download: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed while writing '{}': {error}", archive_path.display()),
            )
        })?;
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256 {
        let _ = fs::remove_file(archive_path);
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CHECKSUM_FAILED",
            format!(
                "PostgreSQL tool archive checksum mismatch: expected {expected_sha256}, got {actual}"
            ),
        ));
    }
    Ok(())
}

fn extract_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), InvocationResponse> {
    let archive_file = File::open(archive_path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to open archive '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut archive = ZipArchive::new(archive_file).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to read archive '{}': {error}",
                archive_path.display()
            ),
        )
    })?;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!("failed to read archive entry {index}: {error}"),
            )
        })?;
        let Some(enclosed_name) = file.enclosed_name() else {
            return Err(InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                "archive contains an unsafe path",
            ));
        };
        let outpath = dest_dir.join(enclosed_name);
        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|error| {
                InvocationResponse::error(
                    "POSTGRES_TOOL_EXTRACT_FAILED",
                    format!(
                        "failed to create directory '{}': {error}",
                        outpath.display()
                    ),
                )
            })?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                InvocationResponse::error(
                    "POSTGRES_TOOL_EXTRACT_FAILED",
                    format!("failed to create directory '{}': {error}", parent.display()),
                )
            })?;
        }
        let mut outfile = File::create(&outpath).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!(
                    "failed to create extracted file '{}': {error}",
                    outpath.display()
                ),
            )
        })?;
        io::copy(&mut file, &mut outfile).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!("failed to extract file '{}': {error}", outpath.display()),
            )
        })?;
    }
    Ok(())
}

fn remove_cache_dir(cache_root: &Path, target: &Path) -> Result<(), InvocationResponse> {
    let cache_root = fs::canonicalize(cache_root).unwrap_or_else(|_| cache_root.to_path_buf());
    let target_abs = if target.exists() {
        fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())
    } else {
        target.to_path_buf()
    };
    if !target_abs.starts_with(&cache_root) {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CLEANUP_FAILED",
            format!(
                "refusing to remove path outside PostgreSQL tool cache: '{}'",
                target.display()
            ),
        ));
    }
    fs::remove_dir_all(target).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CLEANUP_FAILED",
            format!("failed to remove '{}': {error}", target.display()),
        )
    })
}

fn read_tool_config() -> Result<ToolConfig, InvocationResponse> {
    let path = tool_config_path()?;
    if !path.exists() {
        return Ok(ToolConfig {
            version: SETTINGS_VERSION,
            path: None,
        });
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to read '{}': {error}", path.display()),
        )
    })?;
    if raw.trim().is_empty() {
        return Ok(ToolConfig {
            version: SETTINGS_VERSION,
            path: None,
        });
    }
    let config = serde_json::from_str::<ToolConfig>(&raw).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to parse '{}': {error}", path.display()),
        )
    })?;
    if config.version != SETTINGS_VERSION {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!(
                "unsupported postgres tool config version {} in '{}'",
                config.version,
                path.display()
            ),
        ));
    }
    Ok(config)
}

fn write_tool_config(config: &ToolConfig) -> Result<(), InvocationResponse> {
    let path = tool_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_CONFIG_FAILED",
                format!(
                    "failed to create config directory '{}': {error}",
                    parent.display()
                ),
            )
        })?;
    }
    let raw = serde_json::to_string_pretty(config).map_err(|error| {
        InvocationResponse::error(
            "JSON_SERIALIZATION_FAILED",
            format!("failed to serialize postgres tool config: {error}"),
        )
    })?;
    fs::write(&path, raw).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CONFIG_FAILED",
            format!("failed to write '{}': {error}", path.display()),
        )
    })
}

fn tool_config_path() -> Result<PathBuf, InvocationResponse> {
    Ok(config_dir()?.join("postgres-tool.json"))
}

fn managed_tool_path(version: &str) -> Result<PathBuf, InvocationResponse> {
    Ok(postgres_cache_root()?.join(version))
}

fn postgres_cache_root() -> Result<PathBuf, InvocationResponse> {
    Ok(cache_dir()?.join("tools").join("postgres"))
}

fn config_dir() -> Result<PathBuf, InvocationResponse> {
    if let Some(value) = env::var_os("AH_CONFIG_DIR") {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "AH_CONFIG_DIR must not be empty",
            ));
        }
        return Ok(path);
    }
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("AIHelper"))
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve %APPDATA% for postgres plugin config; set AH_CONFIG_DIR",
                )
            })
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("AIHelper")
            })
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve $HOME for postgres plugin config; set AH_CONFIG_DIR",
                )
            })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(value) = env::var_os("XDG_CONFIG_HOME") {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return Ok(path.join("aihelper"));
            }
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join(".config").join("aihelper"))
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve config directory for postgres plugin; set AH_CONFIG_DIR",
                )
            })
    }
}

fn cache_dir() -> Result<PathBuf, InvocationResponse> {
    if let Some(value) = env::var_os("AH_CACHE_DIR") {
        let path = PathBuf::from(value);
        if path.as_os_str().is_empty() {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                "AH_CACHE_DIR must not be empty",
            ));
        }
        return Ok(path);
    }
    #[cfg(target_os = "windows")]
    {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("AIHelper"))
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve %LOCALAPPDATA% for postgres plugin cache; set AH_CACHE_DIR",
                )
            })
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join("Library").join("Caches").join("AIHelper"))
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve $HOME for postgres plugin cache; set AH_CACHE_DIR",
                )
            })
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Some(value) = env::var_os("XDG_CACHE_HOME") {
            let path = PathBuf::from(value);
            if !path.as_os_str().is_empty() {
                return Ok(path.join("aihelper"));
            }
        }
        env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
            .map(|path| path.join(".cache").join("aihelper"))
            .ok_or_else(|| {
                InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "unable to resolve cache directory for postgres plugin; set AH_CACHE_DIR",
                )
            })
    }
}

fn execute_ping(
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let row = match run_json_object::<InfoRow>(context, connection, info_sql(), true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let output = InfoOutput {
        command: "postgres.ping",
        info: row.clone(),
    };
    render_success(
        globals,
        &output,
        format!(
            "ok: PostgreSQL {} database={} user={}\n",
            row.server_version, row.current_database, row.current_user
        ),
    )
}

fn execute_info(
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let row = match run_json_object::<InfoRow>(context, connection, info_sql(), true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let output = InfoOutput {
        command: "postgres.info",
        info: row.clone(),
    };
    render_success(
        globals,
        &output,
        format!(
            "server_version: {}\ndatabase: {}\nuser: {}\nschema: {}\nencoding: {}\n",
            row.server_version,
            row.current_database,
            row.current_user,
            row.current_schema.as_deref().unwrap_or("<none>"),
            row.server_encoding
        ),
    )
}

fn info_sql() -> &'static str {
    "SELECT row_to_json(t)::text FROM (
        SELECT
            current_setting('server_version') AS server_version,
            current_database() AS current_database,
            current_user AS current_user,
            session_user AS session_user,
            current_schema() AS current_schema,
            current_setting('server_encoding') AS server_encoding,
            inet_server_addr()::text AS inet_server_addr,
            inet_server_port() AS inet_server_port
    ) t"
}

fn execute_databases(
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let rows = match run_json_array::<DatabaseRow>(
        context,
        connection,
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                datname AS name,
                pg_get_userbyid(datdba) AS owner,
                pg_encoding_to_char(encoding) AS encoding,
                datallowconn AS allow_connections,
                CASE
                    WHEN has_database_privilege(datname, 'CONNECT')
                      OR pg_has_role('pg_read_all_stats', 'member')
                    THEN pg_size_pretty(pg_database_size(datname))
                    ELSE NULL
                END AS size
            FROM pg_database
            ORDER BY datname
        ) t",
        true,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.databases", rows, render_database_rows)
}

fn execute_schemas(
    args: IncludeSystemArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let filter = if args.include_system {
        ""
    } else {
        "WHERE nspname NOT LIKE 'pg\\_%' AND nspname <> 'information_schema'"
    };
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT nspname AS name, pg_get_userbyid(nspowner) AS owner
            FROM pg_namespace
            {filter}
            ORDER BY nspname
        ) t"
    );
    let rows = match run_json_array::<SchemaRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.schemas", rows, render_schema_rows)
}

fn execute_relations(
    command: &'static str,
    args: RelationListArgs,
    relation_group: &str,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let kinds = if relation_group == "views" {
        "'v', 'm'"
    } else {
        "'r', 'p', 'f', 'm'"
    };
    let system_filter = if args.include_system {
        "TRUE".to_owned()
    } else {
        "n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%'"
            .to_owned()
    };
    let schema_filter = args
        .schema
        .as_deref()
        .map(|schema| format!("AND n.nspname = {}", sql_literal(schema)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                n.nspname AS schema,
                c.relname AS name,
                CASE c.relkind
                    WHEN 'r' THEN 'table'
                    WHEN 'p' THEN 'partitioned-table'
                    WHEN 'f' THEN 'foreign-table'
                    WHEN 'm' THEN 'materialized-view'
                    WHEN 'v' THEN 'view'
                    ELSE c.relkind::text
                END AS kind,
                pg_get_userbyid(c.relowner) AS owner,
                CASE WHEN c.reltuples >= 0 THEN c.reltuples::bigint ELSE NULL END AS rows_estimate,
                pg_size_pretty(pg_total_relation_size(c.oid)) AS size
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relkind IN ({kinds})
              AND {system_filter}
              {schema_filter}
            ORDER BY n.nspname, c.relname
        ) t"
    );
    let rows = match run_json_array::<RelationRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, command, rows, render_relation_rows)
}

fn execute_describe(
    args: DescribeArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let object = match parse_object_name(&args.object) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let relation = match describe_relation(context, connection, &object) {
        Ok(Some(value)) => value,
        Ok(None) => {
            return InvocationResponse::error(
                "POSTGRES_OBJECT_NOT_FOUND",
                format!("PostgreSQL object not found: {}", args.object),
            );
        }
        Err(error) => return error,
    };
    let columns = match describe_columns(context, connection, &relation.schema, &relation.name) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let indexes = match list_indexes(
        context,
        connection,
        Some(&relation.schema),
        Some(&relation.name),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let constraints =
        match describe_constraints(context, connection, &relation.schema, &relation.name) {
            Ok(value) => value,
            Err(error) => return error,
        };
    let output = DescribeOutput {
        command: "postgres.describe",
        relation,
        columns,
        indexes,
        constraints,
    };
    render_success(globals, &output, render_describe_text(&output))
}

fn describe_relation(
    context: &ToolContext,
    connection: &ConnectionArgs,
    object: &ObjectName,
) -> Result<Option<DescribeRelationRow>, InvocationResponse> {
    let schema_filter = if let Some(schema) = &object.schema {
        format!("n.nspname = {}", sql_literal(schema))
    } else {
        "pg_table_is_visible(c.oid)".to_owned()
    };
    let sql = format!(
        "SELECT row_to_json(t)::text FROM (
            SELECT
                n.nspname AS schema,
                c.relname AS name,
                CASE c.relkind
                    WHEN 'r' THEN 'table'
                    WHEN 'p' THEN 'partitioned-table'
                    WHEN 'f' THEN 'foreign-table'
                    WHEN 'm' THEN 'materialized-view'
                    WHEN 'v' THEN 'view'
                    ELSE c.relkind::text
                END AS kind,
                pg_get_userbyid(c.relowner) AS owner,
                CASE WHEN c.reltuples >= 0 THEN c.reltuples::bigint ELSE NULL END AS rows_estimate,
                pg_size_pretty(pg_total_relation_size(c.oid)) AS total_size
            FROM pg_class c
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE c.relkind IN ('r', 'p', 'f', 'm', 'v')
              AND c.relname = {}
              AND {schema_filter}
            ORDER BY n.nspname, c.relname
            LIMIT 1
        ) t",
        sql_literal(&object.name)
    );
    let raw = run_psql_capture(context, connection, &sql, PsqlOutputMode::Json, true)?;
    let trimmed = raw.stdout.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<DescribeRelationRow>(trimmed)
        .map(Some)
        .map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_RESPONSE_INVALID",
                format!("failed to decode describe relation response: {error}"),
            )
        })
}

fn describe_columns(
    context: &ToolContext,
    connection: &ConnectionArgs,
    schema: &str,
    name: &str,
) -> Result<Vec<ColumnRow>, InvocationResponse> {
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                a.attnum AS ordinal,
                a.attname AS name,
                format_type(a.atttypid, a.atttypmod) AS data_type,
                NOT a.attnotnull AS nullable,
                pg_get_expr(ad.adbin, ad.adrelid) AS default,
                col_description(a.attrelid, a.attnum) AS comment
            FROM pg_attribute a
            JOIN pg_class c ON c.oid = a.attrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum
            WHERE n.nspname = {}
              AND c.relname = {}
              AND a.attnum > 0
              AND NOT a.attisdropped
            ORDER BY a.attnum
        ) t",
        sql_literal(schema),
        sql_literal(name)
    );
    run_json_array(context, connection, &sql, true)
}

fn describe_constraints(
    context: &ToolContext,
    connection: &ConnectionArgs,
    schema: &str,
    name: &str,
) -> Result<Vec<ConstraintRow>, InvocationResponse> {
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                con.conname AS name,
                CASE con.contype
                    WHEN 'p' THEN 'primary-key'
                    WHEN 'f' THEN 'foreign-key'
                    WHEN 'u' THEN 'unique'
                    WHEN 'c' THEN 'check'
                    WHEN 'x' THEN 'exclusion'
                    ELSE con.contype::text
                END AS constraint_type,
                pg_get_constraintdef(con.oid, true) AS definition
            FROM pg_constraint con
            JOIN pg_class c ON c.oid = con.conrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = {}
              AND c.relname = {}
            ORDER BY con.conname
        ) t",
        sql_literal(schema),
        sql_literal(name)
    );
    run_json_array(context, connection, &sql, true)
}

fn execute_indexes(
    args: IndexesArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let rows = match list_indexes(
        context,
        connection,
        args.schema.as_deref(),
        args.table.as_deref(),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.indexes", rows, render_index_rows)
}

fn list_indexes(
    context: &ToolContext,
    connection: &ConnectionArgs,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<Vec<IndexRow>, InvocationResponse> {
    let schema_filter = schema
        .map(|schema| format!("AND n.nspname = {}", sql_literal(schema)))
        .unwrap_or_default();
    let table_filter = table
        .map(|table| format!("AND c.relname = {}", sql_literal(table)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                n.nspname AS schema,
                c.relname AS table,
                ci.relname AS name,
                ix.indisprimary AS primary,
                ix.indisunique AS unique,
                pg_get_indexdef(ix.indexrelid) AS definition
            FROM pg_index ix
            JOIN pg_class c ON c.oid = ix.indrelid
            JOIN pg_class ci ON ci.oid = ix.indexrelid
            JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
              {schema_filter}
              {table_filter}
            ORDER BY n.nspname, c.relname, ci.relname
        ) t"
    );
    run_json_array(context, connection, &sql, true)
}

fn execute_extensions(
    args: ExtensionsArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let sql = if args.available {
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                a.name,
                e.extversion AS installed_version,
                a.default_version,
                n.nspname AS schema,
                a.comment
            FROM pg_available_extensions a
            LEFT JOIN pg_extension e ON e.extname = a.name
            LEFT JOIN pg_namespace n ON n.oid = e.extnamespace
            ORDER BY a.name
        ) t"
    } else {
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                e.extname AS name,
                e.extversion AS installed_version,
                a.default_version,
                n.nspname AS schema,
                a.comment
            FROM pg_extension e
            LEFT JOIN pg_available_extensions a ON a.name = e.extname
            LEFT JOIN pg_namespace n ON n.oid = e.extnamespace
            ORDER BY e.extname
        ) t"
    };
    let rows = match run_json_array::<ExtensionRow>(context, connection, sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.extensions", rows, render_extension_rows)
}

fn execute_query(
    args: QueryArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let sql = match resolve_sql(args.sql, args.file, "query") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let sql = match read_only_query_sql(&sql, globals.limit) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let rows = match run_json_value(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let row_count = rows.as_array().map(Vec::len).unwrap_or(0);
    let output = QueryOutput {
        command: "postgres.query",
        row_count,
        rows,
    };
    render_success(globals, &output, render_query_text(&output))
}

fn execute_exec(
    args: ExecArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    if !args.yes {
        return InvocationResponse::error(
            "CONFIRMATION_REQUIRED",
            "exec can change database state; rerun with --yes to confirm",
        );
    }
    let sql = match resolve_sql(args.sql, args.file, "exec") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let output = match run_psql_capture(
        context,
        connection,
        &sql,
        PsqlOutputMode::Raw {
            single_transaction: args.single_transaction,
        },
        false,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let exec_output = ExecOutput {
        command: "postgres.exec",
        stdout: output.stdout,
        stderr: output.stderr,
    };
    render_success(globals, &exec_output, exec_output.stdout.clone())
}

fn execute_explain(
    args: ExplainArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    if args.analyze && !args.yes {
        return InvocationResponse::error(
            "CONFIRMATION_REQUIRED",
            "explain --analyze executes the query; rerun with --yes to confirm",
        );
    }
    let sql = match resolve_sql(args.sql, args.file, "explain") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let cleaned = clean_sql(&sql);
    if cleaned.is_empty() {
        return InvocationResponse::error("INVALID_ARGUMENT", "explain SQL must not be empty");
    }
    let options = explain_options(args.analyze, args.buffers, globals.json);
    let explain_sql = format!("EXPLAIN ({options}) {cleaned}");
    if globals.json {
        let plan = match run_json_value(context, connection, &explain_sql, !args.analyze) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let output = ExplainOutput {
            command: "postgres.explain",
            analyze: args.analyze,
            buffers: args.buffers,
            plan,
        };
        render_success(
            globals,
            &output,
            serde_json::to_string_pretty(&output.plan).unwrap_or_default(),
        )
    } else {
        let output = match run_psql_capture(
            context,
            connection,
            &explain_sql,
            PsqlOutputMode::Raw {
                single_transaction: false,
            },
            !args.analyze,
        ) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let stdout = output.stdout;
        render_success(globals, &stdout, stdout.clone())
    }
}

fn explain_options(analyze: bool, buffers: bool, json: bool) -> String {
    let mut options = Vec::new();
    if json {
        options.push("FORMAT JSON");
    } else {
        options.push("FORMAT TEXT");
    }
    if analyze {
        options.push("ANALYZE TRUE");
    }
    if buffers {
        options.push("BUFFERS TRUE");
    }
    options.join(", ")
}

fn execute_activity(
    args: ActivityArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let mut filters = Vec::new();
    if args.active {
        filters.push("state = 'active'");
    }
    if args.idle_in_tx {
        filters.push("state = 'idle in transaction'");
    }
    let filter = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    let limit = globals.limit.unwrap_or(20).clamp(1, 500);
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                pid,
                usename AS user,
                datname AS database,
                application_name,
                client_addr::text AS client_addr,
                state,
                wait_event_type,
                wait_event,
                query_start::text AS query_start,
                state_change::text AS state_change,
                left(query, 500) AS query
            FROM pg_stat_activity
            {filter}
            ORDER BY query_start NULLS LAST, pid
            LIMIT {limit}
        ) t"
    );
    let rows = match run_json_array::<ActivityRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.activity", rows, render_activity_rows)
}

fn execute_locks(
    args: LocksArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let blocking_filter = if args.blocking {
        "AND blocking.pid IS NOT NULL"
    } else {
        ""
    };
    let limit = globals.limit.unwrap_or(50).clamp(1, 500);
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                blocked.pid AS blocked_pid,
                blocked.usename AS blocked_user,
                blocking.pid AS blocking_pid,
                blocking.usename AS blocking_user,
                blocked_locks.locktype AS lock_type,
                blocked_locks.mode AS mode,
                blocked_locks.relation::regclass::text AS relation,
                left(blocked.query, 500) AS blocked_query,
                left(blocking.query, 500) AS blocking_query
            FROM pg_locks blocked_locks
            JOIN pg_stat_activity blocked ON blocked.pid = blocked_locks.pid
            LEFT JOIN LATERAL unnest(pg_blocking_pids(blocked.pid)) AS blocker(pid) ON TRUE
            LEFT JOIN pg_stat_activity blocking ON blocking.pid = blocker.pid
            WHERE NOT blocked_locks.granted
            {blocking_filter}
            ORDER BY blocked.pid
            LIMIT {limit}
        ) t"
    );
    let rows = match run_json_array::<LockRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.locks", rows, render_lock_rows)
}

fn execute_size(
    args: SizeArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let sql = if let Some(table) = args.table {
        let object = match parse_object_name(&table) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let schema = object.schema.or(args.schema);
        let schema_filter = schema
            .as_deref()
            .map(|schema| format!("AND n.nspname = {}", sql_literal(schema)))
            .unwrap_or_else(|| "AND pg_table_is_visible(c.oid)".to_owned());
        format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
                SELECT
                    'table' AS scope,
                    n.nspname AS schema,
                    c.relname AS name,
                    pg_size_pretty(pg_total_relation_size(c.oid)) AS size,
                    pg_total_relation_size(c.oid) AS bytes
                FROM pg_class c
                JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE c.relname = {}
                  {schema_filter}
                ORDER BY bytes DESC
            ) t",
            sql_literal(&object.name)
        )
    } else if let Some(schema) = args.schema {
        format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
                SELECT
                    'schema' AS scope,
                    n.nspname AS schema,
                    n.nspname AS name,
                    pg_size_pretty(coalesce(sum(pg_total_relation_size(c.oid)), 0)) AS size,
                    coalesce(sum(pg_total_relation_size(c.oid)), 0)::bigint AS bytes
                FROM pg_namespace n
                LEFT JOIN pg_class c ON c.relnamespace = n.oid
                WHERE n.nspname = {}
                GROUP BY n.nspname
            ) t",
            sql_literal(&schema)
        )
    } else {
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT
                'database' AS scope,
                NULL::text AS schema,
                current_database() AS name,
                pg_size_pretty(pg_database_size(current_database())) AS size,
                pg_database_size(current_database()) AS bytes
        ) t"
        .to_owned()
    };
    let rows = match run_json_array::<SizeRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.size", rows, render_size_rows)
}

fn execute_settings(
    args: SettingsArgs,
    context: &ToolContext,
    connection: &ConnectionArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let filter = if args.changed {
        "WHERE source <> 'default'"
    } else {
        ""
    };
    let limit = globals.limit.unwrap_or(100).clamp(1, 1000);
    let sql = format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t)), '[]'::jsonb)::text FROM (
            SELECT name, setting, unit, source, short_desc
            FROM pg_settings
            {filter}
            ORDER BY name
            LIMIT {limit}
        ) t"
    );
    let rows = match run_json_array::<SettingRow>(context, connection, &sql, true) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_rows(globals, "postgres.settings", rows, render_setting_rows)
}

fn read_only_query_sql(sql: &str, limit: Option<usize>) -> Result<String, InvocationResponse> {
    let cleaned = clean_sql(sql);
    if cleaned.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "query SQL must not be empty",
        ));
    }
    if !starts_with_read_only_keyword(&cleaned) {
        return Err(InvocationResponse::error(
            "POSTGRES_QUERY_NOT_READ_ONLY",
            "query accepts only read-only SELECT, WITH, TABLE, or VALUES statements; use exec --yes for mutations",
        ));
    }
    let limited = if let Some(limit) = limit {
        format!(
            "SELECT * FROM ({cleaned}) ah_query LIMIT {}",
            limit.clamp(1, 10000)
        )
    } else {
        cleaned
    };
    Ok(format!(
        "SELECT coalesce(jsonb_agg(row_to_json(ah_query)), '[]'::jsonb)::text FROM ({limited}) ah_query"
    ))
}

fn starts_with_read_only_keyword(sql: &str) -> bool {
    let lower = sql.trim_start().to_ascii_lowercase();
    lower.starts_with("select ")
        || lower.starts_with("select\n")
        || lower == "select"
        || lower.starts_with("with ")
        || lower.starts_with("with\n")
        || lower.starts_with("values ")
        || lower.starts_with("values\n")
        || lower.starts_with("table ")
        || lower.starts_with("table\n")
}

fn resolve_sql(
    sql: Option<String>,
    file: Option<PathBuf>,
    command_name: &str,
) -> Result<String, InvocationResponse> {
    match (sql, file) {
        (Some(sql), None) => Ok(sql),
        (None, Some(path)) => fs::read_to_string(&path).map_err(|error| {
            InvocationResponse::error(
                "FILE_READ_FAILED",
                format!(
                    "failed to read SQL file for {command_name} '{}': {error}",
                    path.display()
                ),
            )
        }),
        (None, None) => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("{command_name} requires --sql TEXT or --file PATH"),
        )),
        (Some(_), Some(_)) => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("{command_name} accepts only one of --sql or --file"),
        )),
    }
}

fn clean_sql(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim().to_owned()
}

#[derive(Debug, Clone)]
struct ObjectName {
    schema: Option<String>,
    name: String,
}

fn parse_object_name(raw: &str) -> Result<ObjectName, InvocationResponse> {
    let parts = split_qualified_identifier(raw)?;
    match parts.as_slice() {
        [name] => Ok(ObjectName {
            schema: None,
            name: name.clone(),
        }),
        [schema, name] => Ok(ObjectName {
            schema: Some(schema.clone()),
            name: name.clone(),
        }),
        _ => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "object name must be NAME or SCHEMA.NAME",
        )),
    }
}

fn split_qualified_identifier(raw: &str) -> Result<Vec<String>, InvocationResponse> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "object name must not be empty",
        ));
    }

    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = trimmed.chars().peekable();
    let mut in_quotes = false;
    let mut quoted_closed = false;
    let mut part_started = false;

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    let _ = chars.next();
                } else {
                    in_quotes = false;
                    quoted_closed = true;
                }
            } else {
                current.push(ch);
            }
            continue;
        }

        match ch {
            '"' if !part_started => {
                in_quotes = true;
                part_started = true;
                quoted_closed = false;
            }
            '"' => return invalid_object_name(),
            '.' => {
                push_identifier_part(&mut parts, &current, quoted_closed)?;
                current.clear();
                part_started = false;
                quoted_closed = false;
            }
            ch if ch.is_whitespace() && !part_started => {}
            ch if ch.is_whitespace() && quoted_closed => {}
            _ if quoted_closed => return invalid_object_name(),
            ch => {
                current.push(ch);
                part_started = true;
            }
        }
    }

    if in_quotes {
        return invalid_object_name();
    }
    push_identifier_part(&mut parts, &current, quoted_closed)?;
    if parts.len() > 2 {
        return invalid_object_name();
    }
    Ok(parts)
}

fn push_identifier_part(
    parts: &mut Vec<String>,
    raw: &str,
    quoted: bool,
) -> Result<(), InvocationResponse> {
    let part = if quoted {
        raw.to_owned()
    } else {
        raw.trim().to_owned()
    };
    if part.is_empty() {
        return invalid_object_name();
    }
    parts.push(part);
    Ok(())
}

fn invalid_object_name<T>() -> Result<T, InvocationResponse> {
    Err(InvocationResponse::error(
        "INVALID_ARGUMENT",
        "object name must be NAME or SCHEMA.NAME",
    ))
}

fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[derive(Debug)]
enum PsqlOutputMode {
    Json,
    Raw { single_transaction: bool },
}

#[derive(Debug)]
struct PsqlOutput {
    stdout: String,
    stderr: String,
}

fn run_json_array<T>(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<Vec<T>, InvocationResponse>
where
    T: DeserializeOwned,
{
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<Vec<T>>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON array response: {error}"),
        )
    })
}

fn run_json_object<T>(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<T>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON object response: {error}"),
        )
    })
}

fn run_json_value(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    read_only: bool,
) -> Result<Value, InvocationResponse> {
    let output = run_psql_capture(context, connection, sql, PsqlOutputMode::Json, read_only)?;
    serde_json::from_str::<Value>(output.stdout.trim()).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_RESPONSE_INVALID",
            format!("failed to decode PostgreSQL JSON response: {error}"),
        )
    })
}

fn run_psql_capture(
    context: &ToolContext,
    connection: &ConnectionArgs,
    sql: &str,
    mode: PsqlOutputMode,
    read_only: bool,
) -> Result<PsqlOutput, InvocationResponse> {
    let mut command = Command::new(&context.psql_path);
    command
        .args(["-X", "-v", "ON_ERROR_STOP=1", "--no-password", "-q"])
        .env("PATH", prepend_path_env(&context.bin_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match mode {
        PsqlOutputMode::Json => {
            command.args(["-t", "-A"]);
        }
        PsqlOutputMode::Raw { single_transaction } => {
            if single_transaction {
                command.arg("--single-transaction");
            }
        }
    }
    apply_connection_env(&mut command, connection, read_only)?;
    command.args(["-c", sql]);
    let output = command.output().map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_PSQL_FAILED",
            format!(
                "failed to execute '{}': {error}",
                context.psql_path.display()
            ),
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(InvocationResponse::error(
            "POSTGRES_PSQL_FAILED",
            format!(
                "psql failed with exit code {:?}: {}",
                output.status.code(),
                truncate_for_error(&stderr, 1200)
            ),
        ));
    }
    Ok(PsqlOutput { stdout, stderr })
}

fn apply_connection_env(
    command: &mut Command,
    connection: &ConnectionArgs,
    read_only: bool,
) -> Result<(), InvocationResponse> {
    if let Some(host) = &connection.host {
        command.env("PGHOST", host);
    }
    if let Some(port) = connection.port {
        command.env("PGPORT", port.to_string());
    }
    if let Some(database) = &connection.database {
        command.env("PGDATABASE", database);
    }
    if let Some(user) = &connection.user {
        command.env("PGUSER", user);
    }
    if let Some(service) = &connection.service {
        command.env("PGSERVICE", service);
    }
    if let Some(sslmode) = &connection.sslmode {
        command.env("PGSSLMODE", sslmode);
    }
    if let Some(password_env) = &connection.password_env {
        let password = env::var(password_env).map_err(|_| {
            InvocationResponse::error(
                "POSTGRES_PASSWORD_ENV_MISSING",
                format!("password environment variable is not set: {password_env}"),
            )
        })?;
        command.env("PGPASSWORD", password);
    }
    if connection.connect_timeout_secs > 0 {
        command.env(
            "PGCONNECT_TIMEOUT",
            connection.connect_timeout_secs.to_string(),
        );
    }
    let mut pgoptions = env::var("PGOPTIONS").unwrap_or_default();
    if let Some(timeout_ms) = connection.statement_timeout_ms {
        append_pgoption(
            &mut pgoptions,
            &format!("-c statement_timeout={}ms", timeout_ms),
        );
    }
    if read_only {
        append_pgoption(&mut pgoptions, "-c default_transaction_read_only=on");
    }
    if !pgoptions.trim().is_empty() {
        command.env("PGOPTIONS", pgoptions);
    }
    Ok(())
}

fn append_pgoption(target: &mut String, value: &str) {
    if !target.trim().is_empty() {
        target.push(' ');
    }
    target.push_str(value);
}

fn prepend_path_env(bin_dir: &Path) -> std::ffi::OsString {
    let old_path = env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin_dir.to_path_buf()];
    paths.extend(env::split_paths(&old_path));
    env::join_paths(paths).unwrap_or(old_path)
}

fn render_rows<T, F>(
    globals: &GlobalOptionsWire,
    command: &'static str,
    rows: Vec<T>,
    text_renderer: F,
) -> InvocationResponse
where
    T: Serialize + Clone,
    F: Fn(&[T]) -> String,
{
    let output = RowsOutput {
        command,
        count: rows.len(),
        rows: rows.clone(),
    };
    render_success(globals, &output, text_renderer(&rows))
}

fn render_success<T: Serialize>(
    globals: &GlobalOptionsWire,
    output: &T,
    text_output: String,
) -> InvocationResponse {
    if globals.quiet {
        return InvocationResponse::ok(None);
    }
    if globals.json {
        match serde_json::to_string_pretty(output) {
            Ok(payload) => InvocationResponse::ok(Some(payload)),
            Err(error) => InvocationResponse::error(
                "JSON_SERIALIZATION_FAILED",
                format!("failed to serialize plugin output: {error}"),
            ),
        }
    } else {
        InvocationResponse::ok(Some(text_output))
    }
}

fn render_tool_status_text(output: &ToolStatusOutput) -> String {
    let mut text = String::new();
    if let Some(selected) = &output.selected {
        text.push_str(&format!(
            "available: true\nsource: {}\npsql: {}\nversion: {}\n",
            selected.source,
            selected
                .psql_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unknown>".to_owned()),
            selected.version_raw.as_deref().unwrap_or("<unknown>")
        ));
    } else {
        text.push_str("available: false\n");
        text.push_str("remediation: ah postgres tool download\n");
    }
    for candidate in &output.candidates {
        if !candidate.accepted {
            text.push_str(&format!(
                "warning: {} candidate '{}' rejected: {}\n",
                candidate.source,
                candidate.path.display(),
                candidate.reason.as_deref().unwrap_or("unknown reason")
            ));
        }
    }
    text
}

fn render_tool_download_text(output: &ToolDownloadOutput) -> String {
    if output.downloaded {
        format!(
            "downloaded PostgreSQL {} toolchain to {}\n",
            output.version,
            output.cache_path.display()
        )
    } else {
        format!(
            "PostgreSQL {} toolchain already exists at {}\n",
            output.version,
            output.cache_path.display()
        )
    }
}

fn render_database_rows(rows: &[DatabaseRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\n",
                row.name,
                row.owner,
                row.encoding,
                row.size.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn render_schema_rows(rows: &[SchemaRow]) -> String {
    rows.iter()
        .map(|row| format!("{}\t{}\n", row.name, row.owner))
        .collect()
}

fn render_relation_rows(rows: &[RelationRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}.{}\t{}\t{}\t{}\n",
                row.schema, row.name, row.kind, row.owner, row.size
            )
        })
        .collect()
}

fn render_index_rows(rows: &[IndexRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}.{}\t{}\tprimary={}\tunique={}\n",
                row.schema, row.table, row.name, row.primary, row.unique
            )
        })
        .collect()
}

fn render_extension_rows(rows: &[ExtensionRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\tinstalled={}\tdefault={}\n",
                row.name,
                row.installed_version.as_deref().unwrap_or("-"),
                row.default_version.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn render_activity_rows(rows: &[ActivityRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}\n",
                row.pid,
                row.user.as_deref().unwrap_or("-"),
                row.database.as_deref().unwrap_or("-"),
                row.state.as_deref().unwrap_or("-"),
                row.query.as_deref().unwrap_or("")
            )
        })
        .collect()
}

fn render_lock_rows(rows: &[LockRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "blocked={}\tblocking={}\t{}\t{}\t{}\n",
                row.blocked_pid,
                row.blocking_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_owned()),
                row.lock_type,
                row.mode,
                row.relation.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

fn render_size_rows(rows: &[SizeRow]) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\n",
                row.scope,
                row.schema
                    .as_ref()
                    .map(|schema| format!("{}.{}", schema, row.name))
                    .unwrap_or_else(|| row.name.clone()),
                row.size
            )
        })
        .collect()
}

fn render_setting_rows(rows: &[SettingRow]) -> String {
    rows.iter()
        .map(|row| format!("{}\t{}\t{}\n", row.name, row.setting, row.source))
        .collect()
}

fn render_describe_text(output: &DescribeOutput) -> String {
    let mut text = format!(
        "{}.{}\t{}\t{}\t{}\n",
        output.relation.schema,
        output.relation.name,
        output.relation.kind,
        output.relation.owner,
        output.relation.total_size
    );
    text.push_str("columns:\n");
    for column in &output.columns {
        text.push_str(&format!(
            "  {}\t{}\tnullable={}\tdefault={}\n",
            column.name,
            column.data_type,
            column.nullable,
            column.default.as_deref().unwrap_or("-")
        ));
    }
    if !output.indexes.is_empty() {
        text.push_str("indexes:\n");
        for index in &output.indexes {
            text.push_str(&format!(
                "  {}\tprimary={}\tunique={}\n",
                index.name, index.primary, index.unique
            ));
        }
    }
    if !output.constraints.is_empty() {
        text.push_str("constraints:\n");
        for constraint in &output.constraints {
            text.push_str(&format!(
                "  {}\t{}\t{}\n",
                constraint.name, constraint.constraint_type, constraint.definition
            ));
        }
    }
    text
}

fn render_query_text(output: &QueryOutput) -> String {
    match serde_json::to_string_pretty(&output.rows) {
        Ok(value) => value,
        Err(_) => format!("{} row(s)\n", output.row_count),
    }
}

fn truncate_for_error(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect::<String>() + "..."
}

fn plugin_manual() -> PluginManual {
    PluginManual {
        plugin_name: PLUGIN_NAME.to_owned(),
        domain: DOMAIN.to_owned(),
        description: DESCRIPTION.to_owned(),
        commands: vec![
            ManualCommand {
                name: "tool status".to_owned(),
                summary: "Show resolved PostgreSQL toolchain status.".to_owned(),
                usage: "tool status".to_owned(),
                examples: vec![manual_example("Inspect selected psql", &["tool", "status"])],
            },
            ManualCommand {
                name: "tool download".to_owned(),
                summary: "Download a managed PostgreSQL toolchain.".to_owned(),
                usage: "tool download [--version VERSION] [--force]".to_owned(),
                examples: vec![manual_example(
                    "Download PostgreSQL 18.4 tools",
                    &["tool", "download", "--version", "18.4"],
                )],
            },
            ManualCommand {
                name: "tool use".to_owned(),
                summary: "Persist an explicit PostgreSQL toolchain path.".to_owned(),
                usage: "tool use --path PATH".to_owned(),
                examples: vec![manual_example(
                    "Use an unpacked PostgreSQL bin directory",
                    &["tool", "use", "--path", "C:\\PostgreSQL\\pgsql\\bin"],
                )],
            },
            ManualCommand {
                name: "tool cleanup".to_owned(),
                summary: "Remove managed PostgreSQL toolchain cache.".to_owned(),
                usage: "tool cleanup [--version VERSION]".to_owned(),
                examples: vec![manual_example(
                    "Remove managed PostgreSQL 18.4 tools",
                    &["tool", "cleanup", "--version", "18.4"],
                )],
            },
            ManualCommand {
                name: "ping".to_owned(),
                summary: "Check PostgreSQL connection.".to_owned(),
                usage: "ping [connection flags] [--ensure-tool]".to_owned(),
                examples: vec![manual_example(
                    "Check local database",
                    &["ping", "--database", "postgres", "--user", "postgres"],
                )],
            },
            ManualCommand {
                name: "info".to_owned(),
                summary: "Show server and session metadata.".to_owned(),
                usage: "info [connection flags]".to_owned(),
                examples: vec![manual_example("Show connection info", &["info"])],
            },
            ManualCommand {
                name: "databases".to_owned(),
                summary: "List databases.".to_owned(),
                usage: "databases [connection flags]".to_owned(),
                examples: vec![manual_example("List databases", &["databases"])],
            },
            ManualCommand {
                name: "schemas".to_owned(),
                summary: "List schemas.".to_owned(),
                usage: "schemas [--include-system]".to_owned(),
                examples: vec![manual_example("List user schemas", &["schemas"])],
            },
            ManualCommand {
                name: "tables".to_owned(),
                summary: "List tables and table-like relations.".to_owned(),
                usage: "tables [--schema NAME] [--include-system]".to_owned(),
                examples: vec![manual_example("List public tables", &["tables", "--schema", "public"])],
            },
            ManualCommand {
                name: "views".to_owned(),
                summary: "List views.".to_owned(),
                usage: "views [--schema NAME] [--include-system]".to_owned(),
                examples: vec![manual_example("List public views", &["views", "--schema", "public"])],
            },
            ManualCommand {
                name: "describe".to_owned(),
                summary: "Describe a table, view, or materialized view.".to_owned(),
                usage: "describe <schema.object>".to_owned(),
                examples: vec![manual_example("Describe a table", &["describe", "public.users"])],
            },
            ManualCommand {
                name: "indexes".to_owned(),
                summary: "List indexes.".to_owned(),
                usage: "indexes [--schema NAME] [--table NAME]".to_owned(),
                examples: vec![manual_example(
                    "List table indexes",
                    &["indexes", "--schema", "public", "--table", "users"],
                )],
            },
            ManualCommand {
                name: "extensions".to_owned(),
                summary: "List installed or available extensions.".to_owned(),
                usage: "extensions [--available]".to_owned(),
                examples: vec![manual_example("List installed extensions", &["extensions"])],
            },
            ManualCommand {
                name: "query".to_owned(),
                summary: "Run a read-only SQL query.".to_owned(),
                usage: "query --sql TEXT|--file PATH [--limit N]".to_owned(),
                examples: vec![manual_example(
                    "Run read-only SQL",
                    &["query", "--sql", "select now() as current_time"],
                )],
            },
            ManualCommand {
                name: "exec".to_owned(),
                summary: "Execute explicit SQL mutations or admin commands.".to_owned(),
                usage: "exec --sql TEXT|--file PATH --yes [--single-transaction]".to_owned(),
                examples: vec![manual_example(
                    "Run an explicit command",
                    &["exec", "--sql", "vacuum analyze", "--yes"],
                )],
            },
            ManualCommand {
                name: "explain".to_owned(),
                summary: "Explain a SQL query plan.".to_owned(),
                usage: "explain --sql TEXT|--file PATH [--analyze --yes] [--buffers]".to_owned(),
                examples: vec![manual_example(
                    "Explain a query",
                    &["explain", "--sql", "select * from pg_class"],
                )],
            },
            ManualCommand {
                name: "activity".to_owned(),
                summary: "Show pg_stat_activity rows.".to_owned(),
                usage: "activity [--active] [--idle-in-tx] [--limit N]".to_owned(),
                examples: vec![manual_example("List active sessions", &["activity", "--active"])],
            },
            ManualCommand {
                name: "locks".to_owned(),
                summary: "Show lock and blocking diagnostics.".to_owned(),
                usage: "locks [--blocking] [--limit N]".to_owned(),
                examples: vec![manual_example("Show blocking locks", &["locks", "--blocking"])],
            },
            ManualCommand {
                name: "size".to_owned(),
                summary: "Show database, schema, or table sizes.".to_owned(),
                usage: "size [--schema NAME] [--table NAME]".to_owned(),
                examples: vec![manual_example("Show current database size", &["size"])],
            },
            ManualCommand {
                name: "settings".to_owned(),
                summary: "Show PostgreSQL settings.".to_owned(),
                usage: "settings [--changed] [--limit N]".to_owned(),
                examples: vec![manual_example("Show changed settings", &["settings", "--changed"])],
            },
        ],
        notes: vec![
            "Uses psql non-interactively with -X, ON_ERROR_STOP=1, and --no-password.".to_owned(),
            "Pass database passwords via --password-env, .pgpass, or libpq service files; never via command argv.".to_owned(),
            "Operational commands do not download tools unless --ensure-tool is provided.".to_owned(),
            "Use global --json for structured machine-readable output.".to_owned(),
        ],
    }
}

fn manual_example(description: &str, argv: &[&str]) -> ManualExample {
    ManualExample {
        description: description.to_owned(),
        argv: argv.iter().map(|item| (*item).to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn manual_examples_parse() {
        let manual = plugin_manual();
        for command in &manual.commands {
            for example in &command.examples {
                let mut args = Vec::with_capacity(example.argv.len() + 1);
                args.push(manual.domain.clone());
                args.extend(example.argv.iter().cloned());
                let parse_result = PostgresCli::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for command '{}': argv={args:?}",
                    command.name
                );
            }
        }
    }

    #[test]
    fn parses_psql_major_version() {
        assert_eq!(parse_psql_major("psql (PostgreSQL) 18.4"), Some(18));
        assert_eq!(parse_psql_major("psql (PostgreSQL) 14.12"), Some(14));
    }

    #[test]
    fn read_only_query_rejects_mutation() {
        let error =
            read_only_query_sql("delete from users", None).expect_err("mutation should fail");
        assert_eq!(
            error.error_code.as_deref(),
            Some("POSTGRES_QUERY_NOT_READ_ONLY")
        );
    }

    #[test]
    fn read_only_query_wraps_limit() {
        let sql = read_only_query_sql("select 1 as value;", Some(5)).expect("query should wrap");
        assert!(sql.contains("LIMIT 5"));
        assert!(sql.contains("jsonb_agg"));
    }

    #[test]
    fn object_name_parses_schema_and_name() {
        let object = parse_object_name("public.users").expect("object should parse");
        assert_eq!(object.schema.as_deref(), Some("public"));
        assert_eq!(object.name, "users");
    }

    #[test]
    fn object_name_handles_quoted_dots() {
        let object =
            parse_object_name(r#""tenant.a"."users.v2""#).expect("quoted object should parse");
        assert_eq!(object.schema.as_deref(), Some("tenant.a"));
        assert_eq!(object.name, "users.v2");
    }

    #[test]
    fn object_name_rejects_unclosed_quote() {
        let error = parse_object_name(r#""public.users"#).expect_err("object should fail");
        assert_eq!(error.error_code.as_deref(), Some("INVALID_ARGUMENT"));
    }

    #[test]
    fn sql_literal_escapes_quotes() {
        assert_eq!(sql_literal("bob's"), "'bob''s'");
    }
}
