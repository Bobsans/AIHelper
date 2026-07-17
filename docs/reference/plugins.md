# `ah plugins`

Runtime plugin management commands.

## `ah plugins list`

List currently registered plugins with source and state.

```bash
ah plugins list [--state <enabled|disabled>] [--json]
```

Output:
- text mode: aligned `DOMAIN`, `PLUGIN`, `SOURCE`, `STATE`, and `DESCRIPTION` columns
- json mode: array of plugin metadata objects with additional fields:
  - `source`: `builtin` or `dynamic`
  - `state`: `enabled` or `disabled`
  - `mcp_exposed`: whether the domain contributes typed MCP tools
  - `mcp_omission_reason`: present when a plugin cannot be exposed over MCP

Example text output:

```text
DOMAIN    PLUGIN             SOURCE   STATE     DESCRIPTION
file      builtin-file       builtin  enabled   File inspection helpers
ollama    external-ollama    dynamic  disabled  Ollama Local API plugin
```

Interactive terminal output uses semantic colors for headings, plugin domains,
sources, and states. Colors are disabled automatically for pipes, redirects,
captured output, and JSON mode. Set `NO_COLOR` to disable colors explicitly.

## `ah plugins disable`

Disable plugin domain (built-in or dynamic).

```bash
ah plugins disable <domain> [--json]
```

Examples:
- `ah plugins disable http`
- `ah plugins disable ollama`

State mutations use a bounded cross-process lock and atomically replace the
global `plugins.json`. If persistence fails, the live and retained in-memory
state is left unchanged; lock contention reports `PERSISTENCE_LOCK_TIMEOUT`.

## `ah plugins enable`

Enable previously disabled plugin domain.

```bash
ah plugins enable <domain> [--json]
```

## `ah plugins reset`

Reset plugin-domain override(s) back to default state.

```bash
ah plugins reset <domain> [--json]
ah plugins reset --all [--json]
```

## Storage

Plugin state is persisted in global user config:
- Windows: `%APPDATA%/AIHelper/plugins.json`
- Linux: `$XDG_CONFIG_HOME/aihelper/plugins.json` (or `$HOME/.config/aihelper/plugins.json`)
- macOS: `$HOME/Library/Application Support/AIHelper/plugins.json`

Override for isolated environments/tests:
- set `AH_CONFIG_DIR` to custom directory

Notes:
- disabled domains return `DOMAIN_DISABLED` on invocation
- invalid dynamic plugins are still skipped at startup; built-in plugins remain available
- dynamic plugins can optionally expose manual data consumed by `ah ai info`
- MCP tools `ah.plugins.list|enable|disable|reset` update the live stdio
  server catalog; clients receive a tool-list-changed notification
- text errors emphasize diagnostic codes and hint labels in interactive terminals
