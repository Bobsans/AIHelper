# Text Formatting Design

## Goal

Add reusable formatting for human-readable CLI output, starting with plugin
management commands and application errors.

Machine-readable output must remain stable and free from ANSI escape sequences.

## Scope

- Introduce a shared text styling mechanism.
- Format `ah plugins list` as an aligned table.
- Style `plugins enable`, `plugins disable`, and `plugins reset` messages.
- Style errors emitted through `AppError`.
- Preserve existing JSON schemas, field names, and values.
- Update plugin reference documentation and relevant tests.

Formatting other domain command output and warnings is intentionally deferred.

## Color Policy

Formatting is enabled independently for stdout and stderr when the target stream
is an interactive terminal.

The `NO_COLOR` environment variable disables colors. JSON output, redirected
output, pipes, and captured test output remain plain text. A user-facing
`--color` option is not added yet.

## Shared Output Mechanism

The existing `output` module will own the formatting policy and semantic styles.
Callers select intent rather than raw ANSI codes:

- heading
- key
- success
- warning
- error
- muted

The mechanism must also support a forced color choice in unit tests so styled
and unstyled rendering can be verified deterministically.

## Plugin List

Text output becomes a table with these columns:

```text
DOMAIN    PLUGIN             SOURCE    STATE      DESCRIPTION
file      builtin-file       builtin   enabled    File inspection helpers
ollama    external-ollama    dynamic   disabled   Ollama Local API plugin
```

Column widths are calculated from plain values before styles are applied, so
ANSI sequences cannot affect alignment.

Semantic styles:

- headings and domains: emphasized key color
- enabled: success
- disabled: error
- dynamic source: key
- builtin source: muted

Empty results retain the existing `no plugins registered` message.

## Plugin State Mutations

Existing messages remain unchanged in plain-text mode. Changed state uses the
success style; idempotent or no-op results use the warning style.

## Errors

All errors printed through `AppError::print` use the shared stderr formatter:

- diagnostic code: emphasized error
- `hint:` label: warning
- message and hint text: unchanged

JSON diagnostics remain unchanged. Plain stderr output outside a terminal must
remain compatible with existing integration tests and shell consumers.

## Testing

- Verify the plugin table structure and alignment in plain captured output.
- Verify JSON plugin output contains no ANSI sequences.
- Unit-test enabled and disabled semantic styles with forced color policy.
- Verify `NO_COLOR` disables automatic formatting.
- Verify plain error output remains compatible with current assertions.
- Verify styled errors color the diagnostic code and hint label.

## Documentation

Update `docs/reference/plugins.md` with the table layout and automatic color
policy. No command syntax or JSON contract changes are required.
