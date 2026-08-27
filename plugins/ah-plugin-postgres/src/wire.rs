//! Every row shape `psql --json` returns, and the payloads this plugin
//! publishes.
//!
//! Declaration only, and deliberately one struct per query rather than one
//! reused bag: the column list *is* the contract, and a query that stops
//! returning a column should fail to deserialise rather than silently produce
//! a null.

use super::*;

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct InfoRow {
    pub(crate) server_version: String,
    pub(crate) current_database: String,
    pub(crate) current_user: String,
    pub(crate) session_user: String,
    pub(crate) current_schema: Option<String>,
    pub(crate) server_encoding: String,
    pub(crate) inet_server_addr: Option<String>,
    pub(crate) inet_server_port: Option<i32>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct InfoOutput {
    pub(crate) command: &'static str,
    #[serde(flatten)]
    pub(crate) info: InfoRow,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct DatabaseRow {
    pub(crate) name: String,
    pub(crate) owner: String,
    pub(crate) encoding: String,
    pub(crate) allow_connections: bool,
    pub(crate) size: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct SchemaRow {
    pub(crate) name: String,
    pub(crate) owner: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct RelationRow {
    pub(crate) schema: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) owner: String,
    pub(crate) rows_estimate: Option<i64>,
    pub(crate) size: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ColumnRow {
    pub(crate) ordinal: i32,
    pub(crate) name: String,
    pub(crate) data_type: String,
    pub(crate) nullable: bool,
    pub(crate) default: Option<String>,
    pub(crate) comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct IndexRow {
    pub(crate) schema: String,
    pub(crate) table: String,
    pub(crate) name: String,
    pub(crate) primary: bool,
    pub(crate) unique: bool,
    pub(crate) definition: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ConstraintRow {
    pub(crate) name: String,
    pub(crate) constraint_type: String,
    pub(crate) definition: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct DescribeRelationRow {
    pub(crate) schema: String,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) owner: String,
    pub(crate) rows_estimate: Option<i64>,
    pub(crate) total_size: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct DescribeOutput {
    pub(crate) command: String,
    pub(crate) relation: DescribeRelationRow,
    pub(crate) columns: Vec<ColumnRow>,
    pub(crate) indexes: Vec<IndexRow>,
    pub(crate) constraints: Vec<ConstraintRow>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ExtensionRow {
    pub(crate) name: String,
    pub(crate) installed_version: Option<String>,
    pub(crate) default_version: Option<String>,
    pub(crate) schema: Option<String>,
    pub(crate) comment: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ActivityRow {
    pub(crate) pid: i32,
    pub(crate) user: Option<String>,
    pub(crate) database: Option<String>,
    pub(crate) application_name: Option<String>,
    pub(crate) client_addr: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) wait_event_type: Option<String>,
    pub(crate) wait_event: Option<String>,
    pub(crate) query_start: Option<String>,
    pub(crate) state_change: Option<String>,
    pub(crate) query: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct LockRow {
    pub(crate) blocked_pid: i32,
    pub(crate) blocked_user: Option<String>,
    pub(crate) blocking_pid: Option<i32>,
    pub(crate) blocking_user: Option<String>,
    pub(crate) lock_type: String,
    pub(crate) mode: String,
    pub(crate) relation: Option<String>,
    pub(crate) blocked_query: Option<String>,
    pub(crate) blocking_query: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct SizeRow {
    pub(crate) scope: String,
    pub(crate) schema: Option<String>,
    pub(crate) name: String,
    pub(crate) size: String,
    pub(crate) bytes: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct SettingRow {
    pub(crate) name: String,
    pub(crate) setting: String,
    pub(crate) unit: Option<String>,
    pub(crate) source: String,
    pub(crate) short_desc: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RowsOutput<T> {
    pub(crate) command: &'static str,
    pub(crate) count: usize,
    pub(crate) rows: Vec<T>,
}

/// A result set the caller's own SQL shapes: rows may be anything.
pub(crate) fn any_array(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type": "array", "items": {}})
}

/// An `EXPLAIN` plan, whose shape PostgreSQL owns and varies by version.
pub(crate) fn any_value(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({})
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryOutput {
    pub(crate) command: &'static str,
    pub(crate) row_count: usize,
    #[schemars(schema_with = "any_array")]
    pub(crate) rows: Value,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecOutput {
    pub(crate) command: &'static str,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplainOutput {
    pub(crate) command: &'static str,
    pub(crate) analyze: bool,
    pub(crate) buffers: bool,
    #[schemars(schema_with = "any_value")]
    pub(crate) plan: Value,
}
