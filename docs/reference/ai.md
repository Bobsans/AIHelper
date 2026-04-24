# `ah ai`

AI-agent focused utility commands.

## `ah ai info`

Print full command manual aggregated from built-in and dynamic plugin manuals.

```bash
ah ai info [--domain <domain>] [--json]
```

Flags:
- `--domain <domain>`: show manual only for one domain (example: `file`, `search`)
- `--json`: emit structured machine-readable manual

Output includes:
- global CLI flags
- host commands (`ai info`, `plugins list`, `plugins enable`, `plugins disable`, `plugins reset`)
- per-plugin command descriptions
- per-command examples intended for AI agents

Notes:
- plugin examples are stored in plugin source code and validated by tests
- dynamic plugins may optionally provide manual via `ah_plugin_manual_json_v1`
