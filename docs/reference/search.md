# `ah search`

Search utilities (text and file discovery).

## `ah search text`

Search by content in files.

```bash
ah search text <pattern> [path] [--glob ...] [--ignore-case] [--context N] [--regex] [--limit N] [--json]
```

Behavior:
- default mode is literal/plain search (`pattern` treated as text)
- add `--regex` to treat `pattern` as regular expression

Flags:
- `--glob <pattern>`: limit files by glob (repeatable)
- `--ignore-case`: case-insensitive matching
- `--context N`: include N lines before/after each match
- `--regex`: enable regex matching mode
- `--limit N`: cap number of returned matches
- `--json`: machine-readable output

Output:
- text mode: one line per hit (`path:line:text`) and optional context lines
- json mode: includes backend, match count, file count, and full match objects

Status: implemented.

## `ah search files`

Search file paths by query substring.

```bash
ah search files <query> [path] [--limit N] [--json]
```

Output:
- text mode: one matched path per line
- json mode: includes backend, match count, and matched paths

Status: implemented.
