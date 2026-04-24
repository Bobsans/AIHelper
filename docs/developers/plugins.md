# Plugin Development

This guide describes how AIHelper plugins integrate with the in-process runtime.

## Plugin Types

- Built-in plugins: compiled into `ah` (current domains use this mode).
- Dynamic plugins: shared libraries loaded at runtime from `plugins` directory next to `ah` executable.

## Runtime Discovery

At startup, runtime scans:

- `<exe-dir>/plugins/*.dll` (Windows)
- `<exe-dir>/plugins/*.so` (Linux)
- `<exe-dir>/plugins/*.dylib` (macOS)

Plugins with duplicate domain names override built-in plugins.
If a dynamic plugin fails to load, the runtime skips it and continues with remaining plugins.
Plugin-domain state (`enabled/disabled`) is managed by host command `ah plugins ...` and persisted in global `plugins.json`.

## ABI Contract

Dynamic plugins must expose symbol:

- `ah_plugin_entry_v1`
- optional: `ah_plugin_manual_json_v1`

Entry returns pointer to:

- `AhPluginApiV1`

Required fields:

- `abi_version`
- `plugin_name`
- `domain`
- `description`
- `invoke_json`
- `free_c_string`

Optional symbol behavior:
- `ah_plugin_manual_json_v1` returns JSON for `PluginManual`
- host uses it for `ah ai info`
- if absent, plugin is still valid and loaded normally

The host validates `abi_version` against `AH_PLUGIN_ABI_VERSION`.

## Invocation Model

Host sends JSON request:

- `InvocationRequest`
  - `domain`
  - `argv`
  - `globals` (`json`, `quiet`, `limit`)

Plugin returns JSON response:

- `InvocationResponse`
  - `success`
  - optional `message`
  - optional `error_code`
  - optional `error_message`

## Best Practices

- Keep plugin command behavior deterministic.
- Do not print partial/broken output on parse failures.
- Use stable error codes for machine handling.
- Treat ABI changes as versioned events (bump API version intentionally).

## Example Dynamic Plugin: Ollama

Repository includes dynamic plugin source at:

- `plugins/ah-plugin-ollama`

Build and install (Windows):

```powershell
cargo build --release -p ah-plugin-ollama
New-Item -ItemType Directory -Force plugins | Out-Null
Copy-Item target/release/ah_plugin_ollama.dll plugins/ah-plugin-ollama.dll
```

Build and install (Linux):

```bash
cargo build --release -p ah-plugin-ollama
mkdir -p plugins
cp target/release/libah_plugin_ollama.so plugins/ah-plugin-ollama.so
```

Build and install (macOS):

```bash
cargo build --release -p ah-plugin-ollama
mkdir -p plugins
cp target/release/libah_plugin_ollama.dylib plugins/ah-plugin-ollama.dylib
```
