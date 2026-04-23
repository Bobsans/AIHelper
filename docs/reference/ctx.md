# `ah ctx`

Context-reduction utilities for AI workflows.

## `ah ctx pack`

Create a compact structured digest of files/directories with lightweight symbol extraction.

```bash
ah ctx pack <path...> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks] [--limit N] [--json]
```

Behavior:
- if no path is provided, current directory is used
- includes line counts and top symbols for text files
- skips binary/non-UTF8 files from deep symbol extraction
- skips large files over `--max-bytes` (default: `8388608`)
- skips symlink targets unless `--follow-symlinks` is set
- `--preset` tunes defaults for context size (`summary` = smallest, `review` = balanced, `debug` = largest)
- explicit `--limit` overrides preset default item count
- JSON output includes skip counters (`skipped_binary_files`, `skipped_large_files`, `skipped_symlink_files`)

Status: implemented.

## `ah ctx symbols`

Extract symbols (functions, classes, headings, etc.) from a file or directory.

```bash
ah ctx symbols <path> [--preset <summary|review|debug>] [--max-bytes BYTES] [--follow-symlinks] [--limit N] [--json]
```

Behavior:
- for directory input, scans files recursively
- `--limit` caps number of files scanned
- supports Rust/Markdown/Python/JS/TS/Vue/Go heuristics
- `--preset` controls default file limit and symbol density per file
- `--max-bytes` skips files larger than limit (default: `8388608`)
- `--follow-symlinks` enables symlink traversal
- JSON output includes skip counters (`skipped_binary_files`, `skipped_large_files`, `skipped_symlink_files`)

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
