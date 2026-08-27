//! The text a command prints when the caller did not ask for JSON.
//!
//! `render_rows` is the shared table: a header, one line per row, and the
//! per-command closures below decide what a column says. The `*_style`
//! functions are the only place a value decides a colour.

use super::*;

pub(crate) fn render_rows<T, F>(
    globals: &GlobalOptionsWire,
    command: &'static str,
    rows: Vec<T>,
    text_renderer: F,
) -> InvocationResponse
where
    T: Serialize + Clone,
    F: Fn(&[T], TextFormatter) -> String,
{
    let output = RowsOutput {
        command,
        count: rows.len(),
        rows: rows.clone(),
    };
    render::render_success(
        globals,
        &output,
        text_renderer(&rows, TextFormatter::stdout()),
    )
}

pub(crate) fn render_tool_status_text(
    output: &ToolStatusOutput,
    formatter: TextFormatter,
) -> String {
    let mut text = String::new();
    if let Some(selected) = &output.selected {
        text.push_str(&format!(
            "{} {}\n{} {}\n{} {}\n{} {}\n",
            formatter.paint(TextStyle::Muted, "available:"),
            formatter.paint(TextStyle::Success, true),
            formatter.paint(TextStyle::Muted, "source:"),
            formatter.paint(TextStyle::Key, selected.source),
            formatter.paint(TextStyle::Muted, "psql:"),
            formatter.paint(
                TextStyle::Key,
                selected
                    .psql_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_owned())
            ),
            formatter.paint(TextStyle::Muted, "version:"),
            formatter.paint(
                TextStyle::Key,
                selected.version_raw.as_deref().unwrap_or("<unknown>")
            )
        ));
    } else {
        text.push_str(&format!(
            "{} {}\n",
            formatter.paint(TextStyle::Muted, "available:"),
            formatter.paint(TextStyle::Warning, false)
        ));
        text.push_str(&format!(
            "{} {}\n",
            formatter.paint(TextStyle::Warning, "remediation:"),
            formatter.paint(TextStyle::Key, "ah postgres tool download")
        ));
    }
    for candidate in &output.candidates {
        if !candidate.accepted {
            text.push_str(&format!(
                "{} {} candidate '{}' rejected: {}\n",
                formatter.paint(TextStyle::Warning, "warning:"),
                formatter.paint(TextStyle::Key, candidate.source),
                formatter.paint(TextStyle::Key, candidate.path.display()),
                formatter.paint(
                    TextStyle::Warning,
                    candidate.reason.as_deref().unwrap_or("unknown reason")
                )
            ));
        }
    }
    text
}

pub(crate) fn render_tool_download_text(
    output: &ToolDownloadOutput,
    formatter: TextFormatter,
) -> String {
    if output.downloaded {
        format!(
            "{} PostgreSQL {} toolchain {} {}\n",
            formatter.paint(TextStyle::Success, "downloaded"),
            formatter.paint(TextStyle::Key, &output.version),
            formatter.paint(TextStyle::Muted, "to"),
            formatter.paint(TextStyle::Key, output.cache_path.display())
        )
    } else {
        format!(
            "PostgreSQL {} toolchain {} {}\n",
            formatter.paint(TextStyle::Key, &output.version),
            formatter.paint(TextStyle::Muted, "already exists at"),
            formatter.paint(TextStyle::Key, output.cache_path.display())
        )
    }
}

pub(crate) fn render_ping_text(row: &InfoRow, formatter: TextFormatter) -> String {
    format!(
        "{} PostgreSQL {} database={} user={}\n",
        formatter.paint(TextStyle::Success, "ok:"),
        formatter.paint(TextStyle::Key, &row.server_version),
        formatter.paint(TextStyle::Key, &row.current_database),
        formatter.paint(TextStyle::Key, &row.current_user)
    )
}

pub(crate) fn render_info_text(row: &InfoRow, formatter: TextFormatter) -> String {
    format!(
        "{} {}\n{} {}\n{} {}\n{} {}\n{} {}\n",
        formatter.paint(TextStyle::Muted, "server_version:"),
        formatter.paint(TextStyle::Key, &row.server_version),
        formatter.paint(TextStyle::Muted, "database:"),
        formatter.paint(TextStyle::Key, &row.current_database),
        formatter.paint(TextStyle::Muted, "user:"),
        formatter.paint(TextStyle::Key, &row.current_user),
        formatter.paint(TextStyle::Muted, "schema:"),
        formatter.paint(
            optional_value_style(row.current_schema.as_deref().unwrap_or("<none>")),
            row.current_schema.as_deref().unwrap_or("<none>")
        ),
        formatter.paint(TextStyle::Muted, "encoding:"),
        formatter.paint(TextStyle::Muted, &row.server_encoding)
    )
}

pub(crate) fn render_database_rows(rows: &[DatabaseRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\n",
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(TextStyle::Muted, &row.owner),
                formatter.paint(TextStyle::Muted, &row.encoding),
                formatter.paint(TextStyle::Muted, row.size.as_deref().unwrap_or("-"))
            )
        })
        .collect()
}

pub(crate) fn render_schema_rows(rows: &[SchemaRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\n",
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(TextStyle::Muted, &row.owner)
            )
        })
        .collect()
}

pub(crate) fn render_relation_rows(rows: &[RelationRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}.{}\t{}\t{}\t{}\n",
                formatter.paint(TextStyle::Key, &row.schema),
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(TextStyle::Heading, &row.kind),
                formatter.paint(TextStyle::Muted, &row.owner),
                formatter.paint(TextStyle::Muted, &row.size)
            )
        })
        .collect()
}

pub(crate) fn render_index_rows(rows: &[IndexRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}.{}\t{}\tprimary={}\tunique={}\n",
                formatter.paint(TextStyle::Key, &row.schema),
                formatter.paint(TextStyle::Key, &row.table),
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(bool_success_style(row.primary), row.primary),
                formatter.paint(bool_success_style(row.unique), row.unique)
            )
        })
        .collect()
}

pub(crate) fn render_extension_rows(rows: &[ExtensionRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\tinstalled={}\tdefault={}\n",
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(
                    if row.installed_version.is_some() {
                        TextStyle::Success
                    } else {
                        TextStyle::Muted
                    },
                    row.installed_version.as_deref().unwrap_or("-")
                ),
                formatter.paint(
                    TextStyle::Muted,
                    row.default_version.as_deref().unwrap_or("-")
                )
            )
        })
        .collect()
}

pub(crate) fn render_activity_rows(rows: &[ActivityRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\t{}\t{}\n",
                formatter.paint(TextStyle::Key, row.pid),
                formatter.paint(
                    optional_value_style(row.user.as_deref().unwrap_or("-")),
                    row.user.as_deref().unwrap_or("-")
                ),
                formatter.paint(
                    optional_value_style(row.database.as_deref().unwrap_or("-")),
                    row.database.as_deref().unwrap_or("-")
                ),
                formatter.paint(
                    activity_state_style(row.state.as_deref().unwrap_or("-")),
                    row.state.as_deref().unwrap_or("-")
                ),
                row.query.as_deref().unwrap_or("")
            )
        })
        .collect()
}

pub(crate) fn render_lock_rows(rows: &[LockRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            let blocking_pid = row
                .blocking_pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_owned());
            format!(
                "blocked={}\tblocking={}\t{}\t{}\t{}\n",
                formatter.paint(TextStyle::Error, row.blocked_pid),
                formatter.paint(
                    if row.blocking_pid.is_some() {
                        TextStyle::Warning
                    } else {
                        TextStyle::Muted
                    },
                    blocking_pid
                ),
                formatter.paint(TextStyle::Muted, &row.lock_type),
                formatter.paint(lock_mode_style(&row.mode), &row.mode),
                formatter.paint(
                    optional_value_style(row.relation.as_deref().unwrap_or("-")),
                    row.relation.as_deref().unwrap_or("-")
                )
            )
        })
        .collect()
}

pub(crate) fn render_size_rows(rows: &[SizeRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            let name = row
                .schema
                .as_ref()
                .map(|schema| format!("{}.{}", schema, row.name))
                .unwrap_or_else(|| row.name.clone());
            format!(
                "{}\t{}\t{}\n",
                formatter.paint(TextStyle::Heading, &row.scope),
                formatter.paint(TextStyle::Key, name),
                formatter.paint(TextStyle::Muted, &row.size)
            )
        })
        .collect()
}

pub(crate) fn render_setting_rows(rows: &[SettingRow], formatter: TextFormatter) -> String {
    rows.iter()
        .map(|row| {
            format!(
                "{}\t{}\t{}\n",
                formatter.paint(TextStyle::Key, &row.name),
                formatter.paint(TextStyle::Key, &row.setting),
                formatter.paint(TextStyle::Muted, &row.source)
            )
        })
        .collect()
}

pub(crate) fn render_describe_text(output: &DescribeOutput, formatter: TextFormatter) -> String {
    let mut text = format!(
        "{}.{}\t{}\t{}\t{}\n",
        formatter.paint(TextStyle::Key, &output.relation.schema),
        formatter.paint(TextStyle::Key, &output.relation.name),
        formatter.paint(TextStyle::Heading, &output.relation.kind),
        formatter.paint(TextStyle::Muted, &output.relation.owner),
        formatter.paint(TextStyle::Muted, &output.relation.total_size)
    );
    text.push_str(&format!(
        "{}\n",
        formatter.paint(TextStyle::Heading, "columns:")
    ));
    for column in &output.columns {
        text.push_str(&format!(
            "  {}\t{}\tnullable={}\tdefault={}\n",
            formatter.paint(TextStyle::Key, &column.name),
            formatter.paint(TextStyle::Heading, &column.data_type),
            formatter.paint(TextStyle::Muted, column.nullable),
            column.default.as_deref().unwrap_or("-")
        ));
    }
    if !output.indexes.is_empty() {
        text.push_str(&format!(
            "{}\n",
            formatter.paint(TextStyle::Heading, "indexes:")
        ));
        for index in &output.indexes {
            text.push_str(&format!(
                "  {}\tprimary={}\tunique={}\n",
                formatter.paint(TextStyle::Key, &index.name),
                formatter.paint(bool_success_style(index.primary), index.primary),
                formatter.paint(bool_success_style(index.unique), index.unique)
            ));
        }
    }
    if !output.constraints.is_empty() {
        text.push_str(&format!(
            "{}\n",
            formatter.paint(TextStyle::Heading, "constraints:")
        ));
        for constraint in &output.constraints {
            text.push_str(&format!(
                "  {}\t{}\t{}\n",
                formatter.paint(TextStyle::Key, &constraint.name),
                formatter.paint(TextStyle::Heading, &constraint.constraint_type),
                constraint.definition
            ));
        }
    }
    text
}

pub(crate) fn optional_value_style(value: &str) -> TextStyle {
    if matches!(value, "-" | "<none>" | "<unknown>") {
        TextStyle::Muted
    } else {
        TextStyle::Key
    }
}

pub(crate) fn bool_success_style(value: bool) -> TextStyle {
    if value {
        TextStyle::Success
    } else {
        TextStyle::Muted
    }
}

pub(crate) fn activity_state_style(state: &str) -> TextStyle {
    match state {
        "active" | "idle in transaction" => TextStyle::Warning,
        "idle in transaction (aborted)" => TextStyle::Error,
        _ => TextStyle::Muted,
    }
}

pub(crate) fn lock_mode_style(mode: &str) -> TextStyle {
    if mode.contains("Exclusive") {
        TextStyle::Warning
    } else {
        TextStyle::Muted
    }
}

pub(crate) fn render_query_text(output: &QueryOutput) -> String {
    match serde_json::to_string_pretty(&output.rows) {
        Ok(value) => value,
        Err(_) => format!("{} row(s)\n", output.row_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_status_renderer_preserves_plain_contract_and_styles_warnings() {
        let output = ToolStatusOutput {
            command: "postgres.tool.status",
            available: false,
            selected: None,
            candidates: vec![CandidateStatus {
                source: "system-path",
                path: PathBuf::from("bad"),
                psql_path: None,
                bin_dir: None,
                version_raw: Some("psql 10".to_owned()),
                version_major: Some(10),
                accepted: false,
                reason: Some("version too old".to_owned()),
            }],
            target_version: DEFAULT_POSTGRES_VERSION,
            minimum_major: MIN_POSTGRES_MAJOR,
            cache_dir: None,
            config_path: None,
            remediation: Some("ah postgres tool download".to_owned()),
        };

        assert_eq!(
            render_tool_status_text(&output, TextFormatter::with_color(false)),
            "available: false\n\
             remediation: ah postgres tool download\n\
             warning: system-path candidate 'bad' rejected: version too old\n"
        );

        let rendered = render_tool_status_text(&output, TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[33mfalse\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[33mwarning:\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[36mah postgres tool download\u{1b}[0m"));
    }

    #[test]
    fn diagnostic_renderers_style_metadata_and_keep_query_text_raw() {
        let activity = ActivityRow {
            pid: 42,
            user: Some("postgres".to_owned()),
            database: Some("app".to_owned()),
            application_name: None,
            client_addr: None,
            state: Some("active".to_owned()),
            wait_event_type: None,
            wait_event: None,
            query_start: None,
            state_change: None,
            query: Some("select * from users".to_owned()),
        };
        let lock = LockRow {
            blocked_pid: 42,
            blocked_user: Some("app".to_owned()),
            blocking_pid: Some(7),
            blocking_user: Some("admin".to_owned()),
            lock_type: "relation".to_owned(),
            mode: "AccessExclusiveLock".to_owned(),
            relation: Some("public.users".to_owned()),
            blocked_query: None,
            blocking_query: None,
        };

        assert_eq!(
            render_activity_rows(
                std::slice::from_ref(&activity),
                TextFormatter::with_color(false)
            ),
            "42\tpostgres\tapp\tactive\tselect * from users\n"
        );
        assert_eq!(
            render_lock_rows(
                std::slice::from_ref(&lock),
                TextFormatter::with_color(false)
            ),
            "blocked=42\tblocking=7\trelation\tAccessExclusiveLock\tpublic.users\n"
        );

        let activity = render_activity_rows(&[activity], TextFormatter::with_color(true));
        assert!(activity.contains("\u{1b}[33mactive\u{1b}[0m"));
        assert!(activity.contains("\tselect * from users\n"));
        assert!(!activity.contains("\u{1b}[36mselect * from users"));

        let lock = render_lock_rows(&[lock], TextFormatter::with_color(true));
        assert!(lock.contains("blocked=\u{1b}[1;31m42\u{1b}[0m"));
        assert!(lock.contains("blocking=\u{1b}[33m7\u{1b}[0m"));
        assert!(lock.contains("\u{1b}[36mpublic.users\u{1b}[0m"));
    }

    #[test]
    fn describe_renderer_preserves_sql_definitions_as_raw_text() {
        let output = DescribeOutput {
            command: "postgres.describe".to_owned(),
            relation: DescribeRelationRow {
                schema: "public".to_owned(),
                name: "users".to_owned(),
                kind: "table".to_owned(),
                owner: "postgres".to_owned(),
                rows_estimate: Some(10),
                total_size: "16 kB".to_owned(),
            },
            columns: vec![ColumnRow {
                ordinal: 1,
                name: "id".to_owned(),
                data_type: "bigint".to_owned(),
                nullable: false,
                default: Some("nextval('users_id_seq'::regclass)".to_owned()),
                comment: None,
            }],
            indexes: vec![IndexRow {
                schema: "public".to_owned(),
                table: "users".to_owned(),
                name: "users_pkey".to_owned(),
                primary: true,
                unique: true,
                definition: "CREATE UNIQUE INDEX users_pkey".to_owned(),
            }],
            constraints: vec![ConstraintRow {
                name: "users_pkey".to_owned(),
                constraint_type: "PRIMARY KEY".to_owned(),
                definition: "PRIMARY KEY (id)".to_owned(),
            }],
        };

        let plain = render_describe_text(&output, TextFormatter::with_color(false));
        assert_eq!(
            plain,
            concat!(
                "public.users\ttable\tpostgres\t16 kB\n",
                "columns:\n",
                "  id\tbigint\tnullable=false\tdefault=nextval('users_id_seq'::regclass)\n",
                "indexes:\n",
                "  users_pkey\tprimary=true\tunique=true\n",
                "constraints:\n",
                "  users_pkey\tPRIMARY KEY\tPRIMARY KEY (id)\n"
            )
        );

        let styled = render_describe_text(&output, TextFormatter::with_color(true));
        assert!(styled.contains("\u{1b}[1;36mcolumns:\u{1b}[0m"));
        assert!(styled.contains("\u{1b}[32mtrue\u{1b}[0m"));
        assert!(styled.contains("default=nextval('users_id_seq'::regclass)"));
        assert!(!styled.contains("\u{1b}[36mnextval"));
        assert!(styled.contains("\tPRIMARY KEY (id)\n"));
        assert!(!styled.contains("\u{1b}[36mPRIMARY KEY (id)"));
    }

    #[test]
    fn query_renderer_remains_raw_json_text() {
        let output = QueryOutput {
            command: "postgres.query",
            row_count: 1,
            rows: serde_json::json!([{ "sql": "select 1" }]),
        };

        let rendered = render_query_text(&output);

        assert!(rendered.contains("\"sql\": \"select 1\""));
        assert!(!rendered.contains('\u{1b}'));
    }
}
