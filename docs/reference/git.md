# `ah git`

Git-focused helpers for compact change analysis.

## `ah git changed`

Show working tree changes from `git status --porcelain`.

```bash
ah git changed [--limit N] [--json]
```

Behavior:
- inside git repo: returns changed entries with statuses
- outside git repo: returns "not a git repository" (or `in_git_repo=false` in JSON)

Status: implemented.

## `ah git diff`

Show current local diff patch.

```bash
ah git diff [--path <path>] [--limit N] [--json]
```

Flags:
- `--path <path>`: restrict diff to a specific file/path
- `--limit N`: cap diff output lines

Status: implemented.

## `ah git blame`

Show blame details for a file.

```bash
ah git blame <path> [--line N] [--limit N] [--json]
```

Flags:
- `--line N`: return blame for one line
- no `--line`: return file blame entries (cap with `--limit`)

Status: implemented.
