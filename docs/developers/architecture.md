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
  - transport-neutral typed command descriptors, effects, execution context,
    and structured errors
- `crates/ah-runtime`:
  - plugin manager
  - built-in + dynamic plugin registry
  - dynamic loader for `.dll/.so/.dylib` plugins
  - typed schema validation and bounded execution abstraction
- `crates/ah-mcp`:
  - dynamic `rmcp` server adapter
  - stdio transport only
  - MCP annotations, risk metadata, cancellation, and tool-list updates

## Runtime Flow

1. CLI parses global options and domain argv.
2. Runtime initializes plugin manager.
3. Built-in plugins are registered
   (`file/search/ctx/git/project/run/http/task`).
4. Dynamic plugins are loaded from `plugins` directory next to `ah` executable (if present).
5. Global plugin-state config is loaded (`plugins.json`) and disabled domains are applied.
6. Domain invocation is routed to plugin:
   - dynamic plugin takes precedence for same domain
   - otherwise built-in plugin handles request
   - disabled domain returns `DOMAIN_DISABLED`
7. Legacy CLI invocation returns `InvocationResponse` (`success/error`).

For `ah mcp serve`, the same bootstrap is followed by:

1. Registering host-only typed commands (`ai.info` and `plugins.*`).
2. Validating every enabled typed command and output schema.
3. Starting the bounded executor and `rmcp` stdio service.
4. Mapping each descriptor to `ah.<command-id>`.
5. Returning validated structured content or a structured diagnostic.

The runtime compiles typed input/output validators into an immutable registry
once per plugin-definition revision. Enabled-state changes have a separate
revision and the MCP adapter swaps complete immutable tool snapshots, so normal
lookup and invocation do not rebuild or serialize the full catalog.

## Plugin Contract

- Host and plugins communicate via `InvocationRequest`/`InvocationResponse` (JSON payload).
- Dynamic plugins expose `ah_plugin_entry_v1` with `AhPluginApiV1`.
- ABI compatibility is validated by runtime (`AH_PLUGIN_ABI_VERSION`).
- Plugin metadata includes:
  - plugin name
  - domain
  - description
  - ABI version

Typed dynamic plugins advertise `typed_commands_v1` and expose the catalog,
invoke, and cancellation sidecar symbols as one complete capability. The
plugin API does not depend on `rmcp`; MCP types remain confined to
`crates/ah-mcp`.

## Command Contracts

- Commands remain domain-scoped (`file`, `search`, `ctx`, `git`, `http`, `task`).
- Global flags (`--json`, `--quiet`, `--cwd`, `--limit`) are converted to plugin wire options.
- Output contract:
  - text by default
  - JSON when `--json`
  - optional suppression with `--quiet`

Typed commands add:

- JSON Schema input and output contracts
- explicit effects, risk, impact, and reversibility metadata
- request-scoped `cwd`, `limit`, deadline, and request id
- structured success, notices, and errors

The current executor is a bounded sequential FIFO. It intentionally prevents
handler overlap while preserving an `Executor` boundary for a future
resource-aware parallel scheduler.

Plugin settings and task stores use a bounded sidecar lock for cross-process
read-modify-write operations and atomically replace complete JSON documents.
In-memory plugin state is published only after persistence succeeds.

## Error Model

- Domain logic returns `AppError` codes.
- Plugin runtime normalizes errors into `InvocationResponse.error_code/error_message`.
- Host converts runtime failures to process-level non-zero exit.
