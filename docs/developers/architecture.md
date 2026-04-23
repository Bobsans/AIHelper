# Architecture Overview

AIHelper now uses a plugin-oriented architecture with in-process runtime dispatch.

## Workspace Layout

- Root crate (`aihelper`):
  - `src/main.rs`: process entrypoint
  - `src/lib.rs`: runtime bootstrap and command dispatch
  - `src/cli.rs`: global option parsing and domain routing
  - `src/plugins.rs`: built-in plugin adapters for core domains
  - `src/commands/*`: domain implementations reused by built-in plugins
- `crates/ah-plugin-api`:
  - stable request/response payload contracts
  - C ABI structures (`AhPluginApiV1`) and symbol constants
- `crates/ah-runtime`:
  - plugin manager
  - built-in + dynamic plugin registry
  - dynamic loader for `.dll/.so/.dylib` plugins

## Runtime Flow

1. CLI parses global options and domain argv.
2. Runtime initializes plugin manager.
3. Built-in plugins are registered (`file/search/ctx/git/task`).
4. Dynamic plugins are loaded from `plugins` directory next to `ah` executable (if present).
5. Domain invocation is routed to plugin:
   - dynamic plugin takes precedence for same domain
   - otherwise built-in plugin handles request
6. Plugin returns `InvocationResponse` (`success/error`).

## Plugin Contract

- Host and plugins communicate via `InvocationRequest`/`InvocationResponse` (JSON payload).
- Dynamic plugins expose `ah_plugin_entry_v1` with `AhPluginApiV1`.
- ABI compatibility is validated by runtime (`AH_PLUGIN_ABI_VERSION`).
- Plugin metadata includes:
  - plugin name
  - domain
  - description
  - ABI version

## Command Contracts

- Commands remain domain-scoped (`file`, `search`, `ctx`, `git`, `task`).
- Global flags (`--json`, `--quiet`, `--cwd`, `--limit`) are converted to plugin wire options.
- Output contract:
  - text by default
  - JSON when `--json`
  - optional suppression with `--quiet`

## Error Model

- Domain logic returns `AppError` codes.
- Plugin runtime normalizes errors into `InvocationResponse.error_code/error_message`.
- Host converts runtime failures to process-level non-zero exit.
