# `ah ctx`

Context-reduction utilities for AI workflows.

## `ah ctx pack`

Create a compact structured digest of files/directories with lightweight symbol extraction.

```bash
ah ctx pack <path...> [--limit N] [--json]
```

Behavior:
- if no path is provided, current directory is used
- includes line counts and top symbols for text files
- skips large/binary-like files from deep symbol extraction

Status: implemented.

## `ah ctx symbols`

Extract symbols (functions, classes, headings, etc.) from a file or directory.

```bash
ah ctx symbols <path> [--limit N] [--json]
```

Behavior:
- for directory input, scans files recursively
- `--limit` caps number of files scanned
- supports Rust/Markdown/Python/JS/TS/Vue/Go heuristics

Status: implemented.

## `ah ctx changed`

Show changed files from git working tree.

```bash
ah ctx changed [--json]
```

Behavior:
- when inside a git repo: returns `git status --porcelain` summary
- outside git repo: returns "not a git repository" (or `in_git_repo=false` in JSON)

Status: implemented.
