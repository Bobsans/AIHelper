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

## `ah git commit-info`

Show commit metadata, touched files, and line stats.

```bash
ah git commit-info [ref] [--limit N] [--json]
```

Text output includes:
- commit hash, author, date, and subject
- file count, additions, and deletions
- changed file paths with status and per-file stats

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

Status: implemented.
