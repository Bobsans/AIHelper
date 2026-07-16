# Run and HTTP Text Formatting Design

## Goal

Apply semantic terminal formatting to `ah run check` and HTTP text reports while
preserving existing plain-text, JSON, and JUnit contracts.

## Scope

- Format the existing `run check` status line and stream headings.
- Format the existing HTTP status fallback line.
- Format HTTP assertion text reports.
- Keep child process output and HTTP response bodies untouched.
- Preserve JSON, JUnit, command syntax, exit codes, and plain captured output.
- Update relevant tests and command reference documentation.

## Run Check Rendering

The existing status line remains:

```text
success=<value> exit_code=<value> timed_out=<value> duration_ms=<value>
```

Interactive terminal styles:

- `success=true`: success
- `success=false`: error
- `timed_out=true`: warning
- `timed_out=false`: muted
- `exit_code` and `duration_ms` tokens: muted
- `stdout:` heading: key
- `stderr:` heading: error

Captured stdout and stderr content remains byte-for-byte unchanged and is never
wrapped in styles.

## HTTP Request Rendering

The existing `HTTP <status> <status-text>` fallback line is styled by status
class:

- 2xx: success
- 3xx: key
- 4xx: warning
- 5xx: error
- other values: muted

No status header is added when a non-empty response body is printed. Response
body content is not inspected or highlighted.

## HTTP Assert Text Rendering

The existing text layout remains:

```text
spec: <path>
PASS <case>
FAIL <case>
  - <failure>
summary: total=<n>, passed=<n>, failed=<n>, duration_ms=<n>
```

Interactive terminal styles:

- `spec:` and case names: key
- `PASS`: success
- `FAIL`: error
- failure bullets: error, with failure text plain
- passed summary token: success
- failed summary token: error when non-zero, muted when zero
- total and duration tokens: muted

JSON and JUnit renderers do not use the text formatter.

## Renderer Structure

Use small pure rendering functions that accept a `TextFormatter` and return
strings. Emission stays in command output adapters. This allows deterministic
forced-color unit tests without terminal capture and keeps domain logic
independent from ANSI policy.

## Testing

- Unit-test colored and plain `run check` status rendering.
- Unit-test HTTP status class mapping.
- Unit-test colored and plain HTTP assert rendering.
- Verify captured integration output contains no ANSI sequences.
- Verify existing JSON and JUnit reports remain unchanged and ANSI-free.
- Run workspace formatting, tests, and build checks.

## Documentation

Update `docs/reference/run.md` and `docs/reference/http.md` with the interactive
formatting and automatic no-color policy.
