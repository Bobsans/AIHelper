# `ah plugins`

Runtime plugin management commands.

## `ah plugins list`

List currently registered plugins.

```bash
ah plugins list [--json]
```

Output:
- text mode: `<domain> (<plugin-name>) - <description>`
- json mode: array of plugin metadata objects

Notes:
- invalid dynamic plugins are skipped at startup; built-in plugins remain available
- dynamic plugins can optionally expose manual data consumed by `ah ai info`
- example dynamic domain: `ollama` from `ah-plugin-ollama` (when installed in `plugins` next to `ah`)
