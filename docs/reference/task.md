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
- task name supports letters, numbers, `-`, `_`, `.`

Status: implemented.

## `ah task list`

List saved tasks.

```bash
ah task list [--limit N] [--json]
```

Status: implemented.

## `ah task run`

Run saved task command through system shell.

```bash
ah task run <name> [--limit N] [--json]
```

Behavior:
- Windows: runs command through `powershell -NoProfile -Command`
- Unix-like: runs command through `sh -lc`
- global `--limit` truncates captured stdout/stderr lines

Status: implemented.
