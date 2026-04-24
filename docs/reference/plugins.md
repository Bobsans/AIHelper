# `ah plugins`

Runtime plugin management commands.

## `ah plugins list`

List currently registered plugins with source and state.

```bash
ah plugins list [--state <enabled|disabled>] [--json]
```

Output:
- text mode: `<domain> (<plugin-name>) [<source>|<state>] - <description>`
- json mode: array of plugin metadata objects with additional fields:
  - `source`: `builtin` or `dynamic`
  - `state`: `enabled` or `disabled`

## `ah plugins disable`

Disable plugin domain (built-in or dynamic).

```bash
ah plugins disable <domain> [--json]
```

Examples:
- `ah plugins disable http`
- `ah plugins disable ollama`

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
