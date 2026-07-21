# MCP `--max-queued` CLI Scope Design

## Scope

Preserve the migration error for the removed MCP option without reserving the
same argument name across unrelated plugin commands.

## Design

Remove the global raw-argv scan. Register a hidden deprecated `--max-queued`
value argument only on the `mcp serve` Clap subcommand and emit the existing
migration error after that subcommand parses.

Plugin arguments and `run check` child arguments remain opaque and are forwarded
unchanged. Both `--max-queued N` and `--max-queued=N` retain the MCP-specific
diagnostic.

## Verification

Cover the MCP migration error, ordinary plugin pass-through, and `run check`
child pass-through.
