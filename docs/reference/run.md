# `ah run`

Command execution helpers for agents.

## `ah run check`

Run an explicit command directly and return a bounded result.

```bash
ah [--json] run check [--timeout-secs SECONDS] [--max-output-bytes BYTES] [--tail-lines N] [--] <command...>
```

Flags:
- `--timeout-secs SECONDS`: kill the command after the timeout (default: 600)
- `--max-output-bytes BYTES`: cap stdout and stderr separately (default: 65536)
- `--tail-lines N`: retain the bounded output suffix, then return its last N lines

Behavior:
- executes the command directly without a shell
- treats the child command and every following token as opaque; use `--` to make the boundary explicit when child arguments resemble `ah` global flags
- place host-global flags such as `--json`, `--quiet`, `--limit`, and `--cwd` before the child command
- captures stdout and stderr separately
- bounds stdout and stderr while they are read, so child output cannot grow memory without limit
- on timeout, terminates the command and its descendant process tree
- reports `success`, `timed_out`, `exit_code`, and `duration_ms`
- `ah` exits successfully even when the checked command fails; inspect `success=false`

Interactive text output uses semantic colors for success, failure, timeout, and
stdout/stderr headings. Child process output is never recolored. Colors are
disabled automatically for pipes, redirects, captured output, and JSON mode.
Set `NO_COLOR` to disable colors explicitly.

Status: implemented.
