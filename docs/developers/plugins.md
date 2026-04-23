# Plugin Development

This guide describes how AIHelper plugins integrate with the in-process runtime.

## Plugin Types

- Built-in plugins: compiled into `ah` (current domains use this mode).
- Dynamic plugins: shared libraries loaded at runtime from `.ah/plugins`.

## Runtime Discovery

At startup, runtime scans:

- `.ah/plugins/*.dll` (Windows)
- `.ah/plugins/*.so` (Linux)
- `.ah/plugins/*.dylib` (macOS)

Plugins with duplicate domain names override built-in plugins.

## ABI Contract

Dynamic plugins must expose symbol:

- `ah_plugin_entry_v1`

Entry returns pointer to:

- `AhPluginApiV1`

Required fields:

- `abi_version`
- `plugin_name`
- `domain`
- `description`
- `invoke_json`
- `free_c_string`

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
