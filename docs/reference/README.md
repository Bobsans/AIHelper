# Command Reference

Reference docs for all command domains and flags.

## Errors and help

Interactive text mode explains invalid input in a human-readable form. When available, it includes a likely correction with a short description of what the corrected command does, the valid command shape, and the most specific `--help` command to run next. Ambiguous corrections can also show a related global command, such as `ah --version`. Operational failures include a practical recovery hint instead of exposing only an internal error code.

Use `--json` when an error must be handled programmatically. JSON errors retain the stable diagnostic fields `domain`, `operation`, `code`, `message`, `cause`, and `exit_code_hint`.

Operational reference:
- [invocation logging](logging.md)

## Domains
- [ai](ai.md)
- [mcp](mcp.md)
- [upgrade](upgrade.md)
- [file](file.md)
- [search](search.md)
- [ctx](ctx.md)
- [git](git.md)
- [project](project.md)
- [run](run.md)
- [task](task.md)
- [http (bundled plugin)](http.md)
- [plugins](plugins.md)
- [github (dynamic plugin)](github.md)
- [gitlab (dynamic plugin)](gitlab.md)
- [ollama (dynamic plugin)](ollama.md)
- [postgres (dynamic plugin)](postgres.md)
