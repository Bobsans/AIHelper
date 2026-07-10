# Recipe: Summarize Git Changes

## Goal
Get compact git context for AI before review or debugging.

## Commands
```bash
ah git changed --json
ah git diff --limit 200
ah git blame <path> --line <n> --json
```

## Example
```bash
ah git changed --json
ah git diff --path src/commands/search.rs --limit 160
ah git blame src/commands/search.rs --line 120 --json
```

## Output Shape
- `git changed --json`: NUL-safe `entries[]` with exact status/path/old_path, including renames and unusual path text
- `git diff --json`: diff text + truncation flag
- `git blame --json`: author/commit metadata per line

## When To Use
- Capture current working-tree summary before task handoff
- Get targeted patch context without opening full file history
- Attribute suspicious line to author/commit quickly
