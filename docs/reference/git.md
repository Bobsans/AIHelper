# `ah git`

Git-focused helpers for compact change analysis.

## `ah git status`

Show a compact repository summary for release or review context.

```bash
ah git status [--json]
```

Text output includes:
- branch and upstream
- ahead/behind counts when upstream exists
- changed, staged, unstaged, and untracked counts
- latest commit subject
- latest reachable tag when available

Interactive output uses semantic colors for repository state, branches,
upstreams, commit hashes, tags, change statuses, and line statistics. Colors
are disabled automatically for pipes, redirects, captured output, and JSON.
Set `NO_COLOR` to disable colors explicitly.

Status: implemented.

## `ah git tags`

List repository tags newest-first.

```bash
ah git tags [--latest] [--limit N] [--json]
```

Flags:
- `--latest`: return only the first tag after sorting
- `--limit N`: cap listed tags

Status: implemented.

## `ah git tag create`

Create one local git tag. This command does not push the tag.

```bash
ah git tag create <tag> [--message TEXT] [--ref REF] [--json]
```

Behavior:
- without `--message`: creates a lightweight tag
- with `--message`: creates an annotated tag
- `--ref` defaults to `HEAD`

Status: implemented.

## `ah git remotes`

List configured remotes with fetch URL, push URL, and provider hint.

```bash
ah git remotes [--json]
```

Provider hints currently recognize GitHub, GitLab, and Bitbucket URL patterns.

Status: implemented.

## `ah git changed`

Show working tree changes from Git porcelain output.

```bash
ah git changed [--limit N] [--json]
```

Behavior:
- inside git repo: returns changed entries with statuses
- outside git repo: returns "not a git repository" (or `in_git_repo=false` in JSON)
- parses NUL-delimited porcelain records, preserving spaces, newlines, literal ` -> ` text, and rename/copy `path` plus `old_path` without heuristic splitting
- interactive status colors distinguish added, modified, untracked, deleted, renamed, and conflict states

Status: implemented.

## `ah git diff`

Show current local diff patch.

```bash
ah git diff [--path <path>] [--limit N] [--json]
```

Flags:
- `--path <path>`: restrict diff to a specific file/path
- `--limit N`: cap diff output lines

Raw diff content is never recolored or otherwise modified.

Status: implemented.

## `ah git commit-info`

Show commit metadata, touched files, and line stats.

```bash
ah git commit-info [ref] [--limit N] [--json]
```

Text output includes:
- commit hash, author, date, and subject
- file count, additions, and deletions
- changed file paths with status and per-file stats

Interactive output colors additions and deletions semantically while preserving
the existing text layout.

`ref` defaults to `HEAD`.

Status: implemented.

## `ah git blame`

Show blame details for a file.

```bash
ah git blame <path> [--line N] [--limit N] [--json]
```

Flags:
- `--line N`: return blame for one line
- no `--line`: return file blame entries (cap with `--limit`)

Line numbers, commit hashes, and authors may be formatted interactively. Blame
source text is never recolored.

Status: implemented.
