//! One function per command, each turning parsed arguments into a response.
//!
//! Every command here is a `psql` invocation with a fixed query and a typed
//! row shape; what varies is the query, the arguments it interpolates, and the
//! renderer for its text form.

use super::*;

pub(crate) fn execute_ping(
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
    render::render_success(
        globals,
        &output,
        render_ping_text(&row, TextFormatter::stdout()),
    )
}

pub(crate) fn execute_info(
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
    render::render_success(
        globals,
        &output,
        render_info_text(&row, TextFormatter::stdout()),
    )
}

pub(crate) fn info_sql() -> &'static str {
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

pub(crate) fn execute_databases(
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

pub(crate) fn execute_schemas(
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

pub(crate) fn execute_relations(
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

pub(crate) fn execute_describe(
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
        command: "postgres.describe".to_owned(),
        relation,
        columns,
        indexes,
        constraints,
    };
    render::render_success(
        globals,
        &output,
        render_describe_text(&output, TextFormatter::stdout()),
    )
}

pub(crate) fn describe_relation(
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

pub(crate) fn describe_columns(
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

pub(crate) fn describe_constraints(
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

pub(crate) fn execute_indexes(
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

pub(crate) fn list_indexes(
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

pub(crate) fn execute_extensions(
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

pub(crate) fn execute_query(
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
    render::render_success(globals, &output, render_query_text(&output))
}

pub(crate) fn execute_exec(
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
    render::render_success(globals, &exec_output, exec_output.stdout.clone())
}

pub(crate) fn execute_explain(
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
        render::render_success(
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
        render::render_success(globals, &stdout, stdout.clone())
    }
}

pub(crate) fn explain_options(analyze: bool, buffers: bool, json: bool) -> String {
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

pub(crate) fn execute_activity(
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

pub(crate) fn execute_locks(
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

pub(crate) fn execute_size(
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

pub(crate) fn execute_settings(
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
