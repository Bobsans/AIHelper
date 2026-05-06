# `ah run`

Command execution helpers for agents.

## `ah run check`

Run an explicit command directly and return a bounded result.

```bash
ah run check [--timeout-secs SECONDS] [--max-output-bytes BYTES] [--tail-lines N] <command...> [--json]
```

Flags:
- `--timeout-secs SECONDS`: kill the command after the timeout (default: 600)
- `--max-output-bytes BYTES`: cap stdout and stderr separately (default: 65536)
- `--tail-lines N`: keep only the last N lines before byte truncation

Behavior:
- executes the command directly without a shell
- captures stdout and stderr separately
- reports `success`, `timed_out`, `exit_code`, and `duration_ms`
- `ah` exits successfully even when the checked command fails; inspect `success=false`

Status: implemented.
