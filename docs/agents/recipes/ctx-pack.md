# Recipe: Pack Context For AI Prompt

## Goal
Produce a compact repo/file digest with symbols instead of raw full-file dumps.

## Commands
```bash
ah ctx pack <path...> --preset review --limit 120 --json
ah ctx symbols <path> --preset summary --limit 80
ah ctx changed --json
```

## Example Workflow
```bash
ah ctx changed --json
ah ctx pack src docs --preset review --limit 100 --json
ah ctx symbols src/commands --preset summary --limit 50
```

## Output Shape
- `ctx pack --json`: `items[]` with `path`, `kind`, `line_count`, `symbol_count`, `symbols`
- `ctx symbols --json`: grouped symbols per file
- `ctx changed --json`: git change entries with statuses

## When To Use
- Build a planning prompt for AI with reduced token usage
- Quickly locate relevant files/symbols before deep reading
- Snapshot current codebase state for debugging sessions
