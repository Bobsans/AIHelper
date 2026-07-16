# File and Context Text Formatting Design

## Goal

Extend the reusable semantic text formatter to the structured text output of the
`file` and `ctx` domains without changing JSON output, redirected output, or raw
file content.

## Scope

### File commands

- `file stat`
  - labels and numeric metadata use the muted style;
  - paths use the key style;
  - directories use the heading style;
  - symlinks and `readonly: true` use the warning style;
  - regular files use the key style;
  - unavailable timestamps remain the literal `null`.
- `file tree`
  - indentation, list markers, and directory suffixes remain unchanged;
  - directories use the heading style;
  - symlinks use the warning style;
  - regular files use the key style.
- `file read`, `file head`, and `file tail`
  - file content remains completely unformatted;
  - existing warnings may use the shared warning formatter.

### Context commands

- `ctx pack`
  - presets and paths use the key style;
  - labels, counters, line numbers, and item statistics use the muted style;
  - item and symbol kinds use the heading style;
  - non-zero skipped counters use the warning style.
- `ctx symbols`
  - presets and file paths use the key style;
  - labels, line numbers, and zero skipped counters use the muted style;
  - symbol kinds use the heading style;
  - non-zero skipped counters use the warning style.
- `ctx changed`
  - a clean result uses the success style;
  - a non-repository result uses the warning style;
  - changed paths use the key style;
  - statuses use success, warning, error, key, or muted styles according to
    their Git-like meaning.

## Output Compatibility

- ANSI sequences are emitted only for interactive terminal text output.
- JSON output remains deterministic and unchanged.
- Redirected, piped, and captured text output remains plain.
- Existing text wording, separators, indentation, ordering, and raw content are
  preserved.

## Validation

- Add unit tests for plain and styled rendering helpers.
- Add integration assertions that captured text output contains no ANSI escape
  sequences.
- Run formatting, workspace tests, and a locked debug build.
- Demonstrate representative `file` and `ctx` commands in an interactive
  terminal.
