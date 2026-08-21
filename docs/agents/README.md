# AI Agent Guide

This folder is optimized for AI-agent workflows with minimal token cost.

## How to Use
- Start from a recipe in `recipes/`.
- Prefer narrow commands with explicit ranges and limits.
- Use `--json` when the result must be parsed by another tool or step.
- Text-mode errors are optimized for people and may include suggestions, usage, and help commands; use `--json` when branching on stable error codes.

## Recipe Index
- [Prepare and publish a release](recipes/release.md)
- [Write changelog entries and GitHub release notes](recipes/release-notes.md)
- [Use AIHelper as an MCP stdio server](recipes/mcp-stdio.md)
- [Run retryable HTTP workflows with extracted variables](recipes/http.md)
- [Check for a trusted AIHelper update](recipes/upgrade-check.md)
- [Read file with line numbers](recipes/file-read.md)
- [Inspect directory tree](recipes/file-tree.md)
- [Search text in files](recipes/search-text.md)
- [Pack context for AI prompt](recipes/ctx-pack.md)
- [Summarize git changes](recipes/git-changes.md)
- [Reuse command recipes](recipes/task-recipes.md)
- [Manage and discover secret references](recipes/secrets.md)
