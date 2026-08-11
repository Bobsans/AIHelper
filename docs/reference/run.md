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

Invocation logs preserve this distinction: the outer record remains
`status: "success"`, while an optional `outcome` stores only child `success`,
`timed_out`, and `exit_code`. Child stdout, stderr, and argv are not copied into
the outcome.

On Windows 10, Windows Server 2016, and newer, native executables are assigned
to a Job Object during process creation so timeout and cancellation cannot race
with descendant startup. Batch scripts retain the same process-tree guarantee.
Non-interactive child processes run without creating visible console windows.

Interactive text output uses semantic colors for success, failure, timeout, and
stdout/stderr headings. Child process output is never recolored. Colors are
disabled automatically for pipes, redirects, captured output, and JSON mode.
Set `NO_COLOR` to disable colors explicitly.

Status: implemented.
