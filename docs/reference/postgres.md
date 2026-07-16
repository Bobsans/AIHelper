# `ah postgres`

Dynamic plugin domain for PostgreSQL database workflows.

This domain is provided by external plugin `ah-plugin-postgres` and is loaded from `plugins` directory next to `ah`.

The plugin uses `psql` non-interactively with `-X`, `ON_ERROR_STOP=1`, and `--no-password`. Pass passwords through `--password-env`, `.pgpass`, or libpq service files; do not pass secrets in command arguments.

Interactive structured output uses semantic colors for tool availability,
versions, paths, database objects, activity states, locks, sizes, and describe
metadata. SQL query results, execution output, explain plans, SQL definitions,
and active query text remain unformatted. Colors are disabled automatically
for pipes, redirects, captured output, and JSON. Set `NO_COLOR` to disable
colors explicitly.

## Toolchain

```bash
ah postgres tool status
ah postgres tool download [--version 18.4] [--force]
ah postgres tool use --path PATH
ah postgres tool cleanup [--version VERSION]
```

Operational commands do not download PostgreSQL tools by default. Use `--ensure-tool` to allow lazy managed download when no acceptable toolchain is resolved.

Tool resolution order:

1. `--tool-path PATH`
2. `AH_POSTGRES_TOOL_PATH`
3. persisted `ah postgres tool use --path PATH`
4. managed AIHelper cache
5. system `PATH`, only if `psql` is new enough

On Windows x64, `tool download` supports the PostgreSQL 18.4 ZIP binary archive from EDB and verifies the pinned SHA256 checksum before extraction.

## Connection Flags

Most operational commands accept:

```bash
--host HOST
--port PORT
--database NAME
--user USER
--service NAME
--sslmode disable|allow|prefer|require|verify-ca|verify-full
--password-env ENV_VAR
--connect-timeout-secs SECONDS
--statement-timeout-ms MILLISECONDS
--ensure-tool
```

## Inspection

```bash
ah postgres ping
ah postgres info
ah postgres databases
ah postgres schemas [--include-system]
ah postgres tables [--schema NAME] [--include-system]
ah postgres views [--schema NAME] [--include-system]
ah postgres describe <schema.object>
ah postgres indexes [--schema NAME] [--table NAME]
ah postgres extensions [--available]
```

## SQL

```bash
ah postgres query --sql TEXT|--file PATH [--limit N]
ah postgres exec --sql TEXT|--file PATH --yes [--single-transaction]
ah postgres explain --sql TEXT|--file PATH [--analyze --yes] [--buffers]
```

`query` wraps statements in JSON output and uses PostgreSQL read-only mode. Use `exec --yes` for mutations and admin commands. `explain --analyze` executes the query, so it requires `--yes`.

## Diagnostics

```bash
ah postgres activity [--active] [--idle-in-tx] [--limit N]
ah postgres locks [--blocking] [--limit N]
ah postgres size [--schema NAME] [--table NAME]
ah postgres settings [--changed] [--limit N]
```

## Output

Text output is compact by default. Use global `--json` for structured machine-readable output.

Stable command identifiers include:

- `postgres.tool.status`
- `postgres.tool.download`
- `postgres.tool.use`
- `postgres.tool.cleanup`
- `postgres.ping`
- `postgres.info`
- `postgres.databases`
- `postgres.schemas`
- `postgres.tables`
- `postgres.views`
- `postgres.describe`
- `postgres.indexes`
- `postgres.extensions`
- `postgres.query`
- `postgres.exec`
- `postgres.explain`
- `postgres.activity`
- `postgres.locks`
- `postgres.size`
- `postgres.settings`

Common stable error codes:

- `POSTGRES_TOOL_UNAVAILABLE`
- `POSTGRES_TOOL_DOWNLOAD_UNSUPPORTED`
- `POSTGRES_TOOL_DOWNLOAD_FAILED`
- `POSTGRES_TOOL_CHECKSUM_FAILED`
- `POSTGRES_PSQL_FAILED`
- `POSTGRES_RESPONSE_INVALID`
- `POSTGRES_QUERY_NOT_READ_ONLY`
- `CONFIRMATION_REQUIRED`
