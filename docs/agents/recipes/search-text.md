# Recipe: Search Text In Files

## Goal
Find relevant code/text snippets quickly without reading full files.

## Commands
```bash
ah search text <pattern> <path> [--context 1] [--limit 50]
ah search text <regex> <path> --regex [--context 1]
```

## Examples
```bash
ah search text "WorkspaceListTable" src --glob "*.vue" --context 1
ah search text "fn\\s+execute" src --regex --context 2
```

## Output Shape
- Text mode: `path:line:text` entries with optional context lines
- JSON mode (`--json`): `matches[]` with path/line/column/text/context

## When To Use
- Locate function/class usage before refactor
- Build minimal context for AI analysis
- Collect targeted snippets for bug investigation
