# Described CLI Suggestions Design

## Goal

Make every `Did you mean` recommendation self-explanatory by showing both the corrected command and a short description of what it does.

## Text output

Suggestions use a two-column layout:

```text
Did you mean:
  ah project version    Show the detected project version
```

When a scoped command can be confused with a global AIHelper action, render a separate alternative:

```text
To show the AIHelper version, run:
  ah --version
```

## Source of descriptions

Use the existing command catalog as the canonical source for plugin command descriptions. This keeps CLI suggestions aligned with MCP and `ah ai info`, including dynamically loaded plugins. Host-level aliases such as `ah --version` and `ah --help` use explicit descriptions because they are flags rather than catalog commands.

If a catalog description is unavailable, keep the command-only recommendation instead of inventing text.

## Compatibility

The change applies only to interactive text rendering. JSON diagnostics, MCP responses, logging fields, exit codes, and plugin ABI contracts remain unchanged.

## Validation

Cover described suggestions for top-level domains and plugin subcommands, the global version alternative, command-only fallback behavior, dynamic catalogs, and unchanged JSON diagnostics.
