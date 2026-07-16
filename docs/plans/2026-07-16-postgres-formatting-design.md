# PostgreSQL Text Formatting Design

## Goal

Apply the shared plugin formatter to structured PostgreSQL text output while
preserving raw SQL, query results, execution output, explain plans, JSON, and
captured text contracts.

## Structured Output

### Tool Commands

- available or downloaded toolchains use the success style;
- missing toolchains and remediation use the warning style;
- rejected candidates and tool errors use the error or warning style;
- versions, executable paths, cache paths, and commands use the key style;
- labels and secondary metadata use the muted style.

### Connection Information

- successful `ping` state uses the success style;
- server versions, database names, users, and schemas use the key style;
- labels, encoding, and secondary connection metadata use the muted style.

### Inspection Rows

- database, schema, relation, index, extension, setting, and object names use
  the key style;
- relation and constraint kinds use the heading style;
- owners, encodings, versions, sizes, sources, and counts use the muted style;
- true primary/unique flags use the success style and false flags use the muted
  style.

### Diagnostics

- activity PIDs and object names use the key style;
- active and idle-in-transaction states use warning styles, normal idle states
  use muted styles, and clearly successful states may use success;
- blocked PIDs use the error style;
- blocking PIDs use the warning style;
- lock types and modes use muted or warning styles;
- relation names use the key style.

### Describe

- relation identity and section headings use heading/key styles;
- column and index names use the key style;
- data and constraint types use the heading style;
- nullable/default metadata uses muted styles;
- SQL defaults, index definitions, constraint definitions, and comments remain
  unformatted source text.

## Raw Output Boundaries

Do not add ANSI formatting to:

- `postgres query` result payloads;
- `postgres exec` stdout or stderr;
- `postgres explain` plans;
- SQL text and SQL definitions;
- active query text returned by `postgres activity`;
- output passed through directly from `psql`.

## Compatibility

- JSON output remains deterministic and unchanged.
- Redirected, piped, and captured text remains plain.
- `NO_COLOR` disables formatting.
- Quiet mode remains silent.
- Existing text wording, ordering, separators, tabs, and line structure remain
  unchanged.

## Validation

- Add plain-contract and forced-color tests for tool, relation, diagnostic, and
  describe renderers.
- Add raw-content assertions for query text and SQL definitions.
- Update the PostgreSQL command reference.
- Run formatting, PostgreSQL plugin tests, workspace tests, locked builds, and
  a safe local TTY demonstration that does not connect to or mutate a database.
