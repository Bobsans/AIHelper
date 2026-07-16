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

## Semantic Text Formatting

Dynamic plugins can use the shared formatter from `ah-plugin-api`:

```rust
use ah_plugin_api::{TextFormatter, TextStyle};

let formatter = TextFormatter::stdout();
let rendered = formatter.paint(TextStyle::Success, "success");
```

`TextFormatter::stdout()` and `TextFormatter::stderr()` enable ANSI styles only
when the corresponding stream is an interactive terminal and `NO_COLOR` is not
set. Piped, redirected, and captured output therefore stays plain without
changing the plugin invocation contract.

Use semantic styles for structured metadata and statuses. Do not format raw
file content, HTTP bodies, model responses, SQL result payloads, CI logs, or
other content intended for downstream processing. JSON output must never
contain ANSI sequences.

Renderer tests can use `TextFormatter::with_color(true)` and
`TextFormatter::with_color(false)` to verify styled and plain contracts
deterministically.

## Managed External Tool Commands

Dynamic plugins that depend on third-party command-line tools must expose a
predictable `tool` command group instead of plugin-specific verbs such as
`install`.

Standard commands:

- `ah <domain> tool status`
  - Show the selected binary or toolchain path, resolver source, detected version, minimum/target version, cache path, companion executable availability, and warnings.
- `ah <domain> tool download [--version VERSION] [--force]`
  - Download and extract a portable/vendor-provided archive into AIHelper's per-user managed cache.
  - This must not perform a system installation, register services, modify registry, or mutate global `PATH`.
- `ah <domain> tool use --path PATH`
  - Persist an explicit user-selected binary or toolchain path for the plugin domain.
- `ah <domain> tool cleanup [--version VERSION]`
  - Remove managed cached tool versions for that plugin domain.
  - This must not delete explicit/user-provided tool paths.

Operational commands may offer `--ensure-tool` to lazily perform the same
download/extract flow when the managed toolchain is missing. Without this flag
or an explicit plugin setting, commands should fail with a clear diagnostic and
suggest `ah <domain> tool download`.

When no acceptable tool is resolved, operational commands must fail before doing
domain work and return a stable missing-tool error such as `TOOL_UNAVAILABLE` or
a domain-specific code like `POSTGRES_TOOL_UNAVAILABLE`. Text output should
include concrete remediation commands, usually `ah <domain> tool download` and
`ah <domain> tool use --path PATH`. JSON output should include the searched
locations, rejected candidate when available, detected version when available,
required minimum/target version, and a remediation command.

If a user provides an explicit path through CLI, environment, or persisted
`tool use`, the plugin must not silently fall back to another tool when that
explicit path is invalid. Explicit user intent should either work or fail with a
clear diagnostic. If the managed cache is missing or corrupt and `--ensure-tool`
is present, the plugin may download or repair the managed toolchain atomically
before continuing.

Tool resolvers should use this order:

1. Explicit CLI path flag.
2. Domain-specific environment variable.
3. Persisted domain setting from `tool use`.
4. Managed AIHelper cache.
5. System `PATH`, only if the detected version is acceptable.

External tool handling must not pass secrets in process argv. It must not modify
global `PATH`; update only the child process environment when a tool needs
adjacent DLL/runtime lookup. Downloads must use an allowlisted HTTPS source,
verify archive integrity when a pinned checksum is available, and extract
atomically under a lock to avoid corrupted caches.

## Best Practices

- Keep plugin command behavior deterministic.
- Do not print partial/broken output on parse failures.
- Use stable error codes for machine handling.
- Treat ABI changes as versioned events (bump API version intentionally).

## Example Dynamic Plugins

Repository includes dynamic plugin sources at:

- `plugins/ah-plugin-github`
- `plugins/ah-plugin-gitlab`
- `plugins/ah-plugin-ollama`
- `plugins/ah-plugin-postgres`

Build and install one plugin (Windows):

```powershell
cargo build --release -p ah-plugin-github
New-Item -ItemType Directory -Force plugins | Out-Null
Copy-Item target/release/ah_plugin_github.dll plugins/ah-plugin-github.dll
```

Build and install one plugin (Linux):

```bash
cargo build --release -p ah-plugin-github
mkdir -p plugins
cp target/release/libah_plugin_github.so plugins/ah-plugin-github.so
```

Build and install one plugin (macOS):

```bash
cargo build --release -p ah-plugin-github
mkdir -p plugins
cp target/release/libah_plugin_github.dylib plugins/ah-plugin-github.dylib
```
