# `ah task`

Reusable workflow recipes for repetitive operations.

## `ah task save`

Save or update a reusable command recipe.

```bash
ah task save <name> <command> [--json]
```

Behavior:
- tasks are stored in local project file: `.ah/tasks.json`
- save is upsert by task name
- concurrent saves are serialized per store and replace the JSON document
  atomically, preserving unrelated task updates
- task name supports letters, numbers, `-`, `_`, `.`
- interactive text output highlights successful saves, task names, and stored commands

Status: implemented.

## `ah task list`

List saved tasks.

```bash
ah task list [--limit N] [--json]
```

Task names and commands use semantic terminal formatting. Colors are disabled
for pipes, redirects, captured output, JSON, and when `NO_COLOR` is set.

Status: implemented.

## `ah task run`

Run saved task command through system shell.

```bash
ah task run <name> [--timeout-secs SECONDS] [--max-output-bytes BYTES] [--limit N] [--json]
```

Behavior:
- Windows: runs command through `powershell -NoProfile -Command`
- Unix-like: runs command through `sh -lc`
- `--timeout-secs` terminates the task process tree after the deadline (default: `600`)
- `--max-output-bytes` bounds stdout and stderr separately while reading (default: `65536`)
- global `--limit` truncates captured stdout/stderr lines
- timeout returns `TASK_TIMEOUT`; byte or line truncation sets `truncated=true`
- executed task stdout and stderr are never recolored or otherwise modified

Status: implemented.
