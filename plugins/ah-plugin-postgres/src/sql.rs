//! Building SQL, and refusing to build the wrong SQL.
//!
//! This is the security-relevant file. Two rules live here and nowhere else:
//! a read-only command's statement has to *start* with a read-only keyword,
//! and an object name is split into its parts and quoted part by part rather
//! than interpolated - so a schema or table called `x"; drop table y; --`
//! becomes one quoted identifier instead of two statements.

use super::*;

pub(crate) fn read_only_query_sql(
    sql: &str,
    limit: Option<usize>,
) -> Result<String, InvocationResponse> {
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

pub(crate) fn starts_with_read_only_keyword(sql: &str) -> bool {
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

pub(crate) fn resolve_sql(
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

pub(crate) fn clean_sql(sql: &str) -> String {
    sql.trim().trim_end_matches(';').trim().to_owned()
}

#[derive(Debug, Clone)]
pub(crate) struct ObjectName {
    pub(crate) schema: Option<String>,
    pub(crate) name: String,
}

pub(crate) fn parse_object_name(raw: &str) -> Result<ObjectName, InvocationResponse> {
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

pub(crate) fn split_qualified_identifier(raw: &str) -> Result<Vec<String>, InvocationResponse> {
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

pub(crate) fn push_identifier_part(
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

pub(crate) fn invalid_object_name<T>() -> Result<T, InvocationResponse> {
    Err(InvocationResponse::error(
        "INVALID_ARGUMENT",
        "object name must be NAME or SCHEMA.NAME",
    ))
}

pub(crate) fn sql_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
