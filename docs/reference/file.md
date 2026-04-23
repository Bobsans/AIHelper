# `ah file`

File utilities for reading and inspecting files.

## `ah file read`

Read full file or selected line range.

```bash
ah file read <path> [-n] [--from N] [--to N] [--limit N] [--json]
```

Flags:
- `-n`, `--number-lines`: prepend line numbers
- `--from N`: start line (1-based)
- `--to N`: end line (1-based)
- `--limit N`: cap number of returned lines
- `--json`: machine-readable output

Status: implemented.

## `ah file head`
Read the first file lines.

```bash
ah file head <path> [--lines N] [-n] [--limit N] [--json]
```

Flags:
- `--lines N`: number of lines to return (default: `20`)
- `-n`, `--number-lines`: prepend source line numbers
- `--limit N`: cap number of returned lines
- `--json`: machine-readable output

Status: implemented.

## `ah file tail`
Read the last file lines.

```bash
ah file tail <path> [--lines N] [-n] [--limit N] [--json]
```

Flags:
- `--lines N`: number of lines to return (default: `20`)
- `-n`, `--number-lines`: prepend source line numbers
- `--limit N`: cap number of returned lines
- `--json`: machine-readable output

Status: implemented.

## `ah file stat`
Read basic metadata for a file or directory.

```bash
ah file stat <path> [--json]
```

Text output fields:
- `path`
- `kind` (`file`, `directory`, `symlink`, `other`)
- `size_bytes`
- `readonly`
- `modified_unix_seconds`
- `created_unix_seconds`

Status: implemented.

## `ah file tree`
Render a directory tree (or a single file node).

```bash
ah file tree [path] [--depth N] [--limit N] [--json]
```

Flags:
- `path`: target directory or file (default: current directory)
- `--depth N`: recursion depth from root (`0` means root only)
- `--limit N`: cap number of returned entries
- `--json`: machine-readable output with flattened `entries`

Status: implemented.
