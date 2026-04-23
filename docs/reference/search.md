# `ah search`

Search utilities (text and file discovery).

## `ah search text`

Search by content in files.

```bash
ah search text <pattern> [path] [--glob ...] [--ignore-case] [--context N] [--regex] [--max-bytes BYTES] [--follow-symlinks] [--limit N] [--json]
```

Behavior:
- default mode is literal/plain search (`pattern` treated as text)
- add `--regex` to treat `pattern` as regular expression

Flags:
- `--glob <pattern>`: limit files by glob (repeatable)
- `--ignore-case`: case-insensitive matching
- `--context N`: include N lines before/after each match
- `--regex`: enable regex matching mode
- `--max-bytes BYTES`: skip files larger than limit while scanning (default: `8388608`)
- `--follow-symlinks`: follow symlink directories/files during traversal
- `--limit N`: cap number of returned matches
- `--json`: machine-readable output

Safety behavior:
- binary/non-UTF8 files are skipped
- large files are skipped by `--max-bytes`
- symlinks are skipped unless `--follow-symlinks` is set

Output:
- text mode: one line per hit (`path:line:text`) and optional context lines
- json mode: includes backend, match count, file count, full match objects, and skip counters

Status: implemented.

## `ah search files`

Search file paths by query substring.

```bash
ah search files <query> [path] [--follow-symlinks] [--limit N] [--json]
```

Flags:
- `--follow-symlinks`: follow symlink directories/files during traversal

Output:
- text mode: one matched path per line
- json mode: includes backend, match count, and matched paths

Status: implemented.
