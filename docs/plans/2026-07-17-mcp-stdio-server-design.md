# MCP Stdio Server Design

## Status

Approved for implementation planning on 2026-07-17.

## Goal

Expose AIHelper as a local Model Context Protocol server through:

```text
ah mcp serve
```

Every supported AIHelper command is represented as a separate typed MCP tool
with explicit input and output JSON Schema, structured results, stable errors,
and machine-readable impact metadata.

The design targets MCP protocol revision `2025-11-25` and uses the official
Rust SDK, `rmcp`, for protocol lifecycle and stdio transport.

## Decisions

- Support stdio only.
- Do not implement Streamable HTTP, SSE, OAuth, or remote server operation.
- Expose each command as a separate typed tool.
- Publish all read-only and mutating commands.
- Do not restrict filesystem access to a workspace root. Commands may use any
  path available to the AIHelper process.
- Describe effects through standard MCP tool annotations, the tool description,
  and namespaced AIHelper metadata.
- Require every successful tool call to return `structuredContent` conforming
  to an `outputSchema`.
- Keep v1 execution synchronous and sequential.
- Keep the execution boundary ready for bounded parallel execution and MCP
  Tasks in a future protocol version.
- Expose a dynamic plugin through MCP only when it implements the complete
  typed command contract.
- Preserve the existing CLI JSON fields, plugin ABI v1 layout, and legacy
  invocation contracts.

## Non-Goals

- Network transports or remote deployment.
- Authentication or authorization.
- Server-side approval prompts for mutating commands.
- MCP resources, prompts, sampling, elicitation, or Tasks in v1.
- Runtime discovery of newly copied plugin libraries without restarting the
  server.
- Automatic MCP exposure for legacy plugins based on CLI usage strings.
- A schema-to-argv or subprocess wrapper as the production architecture.

## Current Constraints

The existing plugin contract is CLI-oriented:

- `InvocationRequest` contains `domain`, `argv`, and global flags.
- `InvocationResponse` contains an optional string message or diagnostic.
- `PluginManual` contains usage strings and examples, but no JSON Schema.
- Built-in command implementations may print directly to stdout.
- Dynamic plugins return text or serialized JSON inside the response message.
- CLI startup can change the process-wide current directory.

These behaviors are incompatible with a reliable stdio MCP server. MCP stdout
must contain only JSON-RPC messages, tool inputs and outputs must be typed, and
per-request state must remain isolated for future concurrency.

## Architecture

AIHelper gains a protocol-independent typed command kernel. CLI and MCP become
two frontends over the same command catalog and handlers.

```text
CLI argv ───────► CLI adapter ───┐
                                 │
                                 ▼
                          CommandService
                                 │
                                 ▼
                         SequentialExecutor
                                 │
                    ┌────────────┴────────────┐
                    ▼                         ▼
           Built-in commands         Dynamic typed plugins
                    ▲                         ▲
                    └────────────┬────────────┘
                                 │
                          Command catalog
                                 │
                                 ▼
MCP JSON ───────► MCP stdio adapter
```

### `ah-plugin-api`

The plugin API owns protocol-independent wire contracts:

- `CommandCatalog`
- `CommandDescriptor`
- `CommandEffects`
- `CommandExample`
- `TypedInvocationRequest`
- `ExecutionContextWire`
- `TypedInvocationResponse`
- `CommandNotice`
- `CommandError`

It does not depend on `rmcp` and does not expose MCP SDK types to plugins.

### `ah-runtime`

The runtime:

- aggregates host, built-in, and dynamic command descriptors;
- filters disabled plugin domains;
- validates typed requests and responses;
- routes typed invocations;
- owns the executor abstraction;
- exposes catalog change events to frontends.

The initial implementation is `SequentialExecutor`. The executor API accepts
request IDs, deadlines, and cancellation handles so a later
`BoundedConcurrentExecutor` does not require command contract changes.

### `crates/ah-mcp`

A new workspace crate owns MCP-specific behavior:

- `rmcp` stdio lifecycle;
- MCP initialization and capabilities;
- `tools/list`;
- `tools/call`;
- cancellation notifications;
- command descriptor to MCP tool conversion;
- MCP result and error conversion;
- `notifications/tools/list_changed`.

The crate depends on `ah-plugin-api` and `ah-runtime`. The runtime does not
depend on the MCP crate.

### Root Crate

The root crate:

- registers built-in and host commands;
- initializes the shared `CommandService`;
- retains the CLI frontend;
- adds the `mcp serve` host command;
- starts the MCP stdio service.

## Command Catalog

### Stable IDs and MCP Names

Internal command IDs retain the existing domain hierarchy:

```text
file.read
search.text
github.issue.create
postgres.tool.download
```

MCP tool names add the `ah.` namespace:

```text
ah.file.read
ah.search.text
ah.github.issue.create
ah.postgres.tool.download
```

Nested CLI command groups become additional dot-separated segments. Names are
case-sensitive, deterministic, and limited to MCP-compatible ASCII characters.

All callable operational and host commands are published. `mcp.serve` is
explicitly CLI-only because invoking a transport launcher through its own MCP
connection is not meaningful.

### Descriptor

Each command provides a descriptor equivalent to:

```rust
pub struct CommandDescriptor {
    pub id: String,
    pub title: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    pub effects: CommandEffects,
    pub examples: Vec<CommandExample>,
}
```

Built-in descriptors generate schemas from the same serde DTOs used by command
handlers, using `schemars::JsonSchema`. Dynamic plugins provide serialized JSON
Schema through their command catalog.

Schemas use JSON Schema 2020-12, require an object at the root, describe every
field, and reject unknown properties unless a command explicitly accepts an
open map.

`context` is a reserved top-level MCP input property. Command DTOs must not
define a field with that name.

CLI-specific usage rendering may remain separate where positional argument
syntax cannot be derived without ambiguity. Command identity, summaries, input
semantics, output semantics, defaults, and effects come from the typed command
definition.

## Input Contract

Each MCP tool input combines command arguments with an optional common context:

```json
{
  "path": "src/lib.rs",
  "context": {
    "cwd": "D:/Work/DarkBoy/AIHelper",
    "limit": 100,
    "timeout_ms": 30000
  }
}
```

The MCP adapter separates `context` from the command DTO before invoking the
runtime.

Server-controlled context fields include:

- request ID;
- cancellation handle;
- accepted-at timestamp;
- effective deadline.

Caller-controlled context fields include:

- `cwd`: base directory for relative paths;
- `limit`: optional output item or line limit;
- `timeout_ms`: total queue and execution deadline.

For an in-process built-in command, the runtime converts the deadline to a
monotonic timer. For a dynamic plugin, `ExecutionContextWire` contains:

```json
{
  "request_id": "opaque-request-id",
  "cwd": "D:/Work/DarkBoy/AIHelper",
  "limit": 100,
  "remaining_timeout_ms": 27500
}
```

The host calculates `remaining_timeout_ms` immediately before typed invocation,
so time already spent in the queue is not restored. The plugin creates its own
monotonic deadline from that remaining duration.

The startup working directory is used when `context.cwd` is absent. A global
CLI `--limit` supplies the server default when the tool call omits a limit.

Per-request execution never calls `std::env::set_current_dir`. Built-in
operations resolve paths against the explicit context. Child processes receive
their working directory through `Command::current_dir`.

No path sandbox is applied by the MCP layer. Normal operating-system process
permissions remain the access boundary.

## Output Contract

Every successful command returns a JSON object:

```rust
pub struct CommandResult {
    pub data: serde_json::Value,
    pub text: Option<String>,
    pub notices: Vec<CommandNotice>,
}
```

`data` must be an object conforming to the command `outputSchema`.

The MCP adapter returns:

- `structuredContent`: the validated `data` object;
- `content`: one text block containing compact serialized JSON;
- `isError`: absent or `false`;
- namespaced execution metadata in `_meta` when present.

The CLI `--json` frontend prints the same `data` object, preserving released
field names and nesting. Human-readable CLI output is produced by a separate
renderer and never forms the runtime result contract.

The runtime validates plugin output before returning it. Invalid output becomes
`OUTPUT_SCHEMA_VIOLATION`; invalid structured data is never sent to the model.

## Error Model

Protocol-level errors are limited to conditions such as:

- malformed JSON-RPC;
- unsupported MCP method;
- unknown MCP tool.

Input validation, command failures, dependency failures, timeouts, disabled
domains, and plugin failures are tool execution errors with `isError: true`.

The text content contains a concise actionable message. Namespaced metadata
contains the stable diagnostic:

```json
{
  "dev.aihelper/diagnostic": {
    "code": "DEPENDENCY_MISSING",
    "message": "Required external tool not found: git",
    "domain": "git",
    "operation": "git.status",
    "cause": "Local git commands require git on PATH",
    "retryable": false
  }
}
```

Error results do not need to conform to the success `outputSchema` and omit
`structuredContent`. Existing `ErrorDiagnostic` codes and released CLI error
behavior remain stable.

## Effect and Risk Metadata

`CommandEffects` records:

- whether the command is read-only;
- whether it may make destructive changes;
- whether repeated execution is idempotent;
- whether it interacts with an open external world;
- effect categories such as filesystem read/write/delete, process spawn,
  network read/write, configuration mutation, and external system mutation;
- risk level: `low`, `medium`, `high`, or `critical`;
- human-readable impact;
- reversibility: `yes`, `no`, or `unknown`.

The MCP adapter maps the standard fields to:

```json
{
  "annotations": {
    "readOnlyHint": false,
    "destructiveHint": true,
    "idempotentHint": false,
    "openWorldHint": true
  }
}
```

It also emits:

```json
{
  "_meta": {
    "dev.aihelper/risk": {
      "level": "high",
      "effects": ["external.write"],
      "impact": "May modify issues in a remote repository",
      "reversible": "unknown"
    }
  }
}
```

The impact sentence is included in the standard tool description because MCP
clients may ignore custom metadata. These values are informational. AIHelper
does not add a server-side confirmation gate.

Commands capable of arbitrary process or shell execution are not classified as
read-only, even when their common use is running checks.

## Stdio Lifecycle

`ah mcp serve` performs the normal plugin bootstrap, applies plugin settings,
builds the typed catalog, and starts `rmcp` over stdin/stdout.

The server declares:

```json
{
  "capabilities": {
    "tools": {
      "listChanged": true
    }
  }
}
```

No MCP server capabilities beyond tools are declared in v1.

Stdout belongs exclusively to MCP JSON-RPC. Startup information, diagnostics,
plugin warnings, and logs go to stderr. ANSI formatting is disabled for the MCP
process path.

The CLI interface is:

```text
ah mcp serve [--max-queued N] [--default-timeout-ms N]
```

Defaults:

- `--max-queued 32`
- `--default-timeout-ms 300000`

Global `--cwd`, `--limit`, and `--quiet` configure default execution context
and stderr verbosity. `--json` is rejected for `mcp serve` because stdout is
already a JSON-RPC transport.

## Execution and Cancellation

The stdio I/O loop and execution worker are separate. While a tool runs, the
server continues to accept:

- pings;
- cancellation notifications;
- new requests for the bounded queue.

The v1 queue is FIFO and the executor runs one handler at a time. The timeout
starts when the request is accepted, so queue wait counts against the deadline.

Cancellation behavior:

- a queued request is removed before execution;
- an active request activates its cancellation token;
- built-in handlers must poll the token or use cancellation-aware operations;
- child processes are terminated as a process group where supported;
- HTTP and database operations receive explicit deadlines;
- a cancelled or expired result is not returned as a successful tool result.

An AIHelper deadline produces a `TIMEOUT` tool execution error. A client
cancellation notification follows the MCP cancellation lifecycle and suppresses
any late successful result.

Typed dynamic plugins must implement a thread-safe cancellation control path.
The host cannot safely force-stop arbitrary in-process plugin code. Failure to
honor cancellation is a plugin contract violation and is reported
deterministically, but the host does not terminate its own process to enforce
it.

## Dynamic Plugin Extension

`AhPluginApiV1`, `InvocationRequest`, `InvocationResponse`, and `invoke_json`
remain unchanged.

Plugin API minor version increases from `1.0` to `1.1`. The new capability is:

```text
typed_commands_v1
```

Dynamic plugins expose optional sidecar symbols:

```text
ah_plugin_command_catalog_json_v1
ah_plugin_invoke_command_json_v1
ah_plugin_cancel_command_v1
```

Conceptual signatures:

```rust
pub type AhPluginCommandCatalogJsonV1 =
    unsafe extern "C" fn() -> *mut c_char;

pub type AhPluginInvokeCommandJsonV1 =
    unsafe extern "C" fn(request_json: *const c_char) -> *mut c_char;

pub type AhPluginCancelCommandV1 =
    unsafe extern "C" fn(request_id: *const c_char) -> i32;
```

Owned JSON strings are freed through the existing plugin `free_c_string`
function. The invoke and cancel symbols must be safe to call concurrently.

A plugin is MCP-capable only when:

- metadata declares `typed_commands_v1`;
- API version is compatible;
- all three symbols exist;
- its catalog is valid;
- every descriptor has valid schemas and effect metadata.

An API `1.0` plugin continues to load and work through CLI invocation but is
omitted from the MCP catalog. A partially implemented typed extension is
rejected with a deterministic plugin load warning.

## Live Plugin State

The MCP catalog contains enabled typed domains only.

Calling plugin state tools updates both persisted settings and the live server:

- disabling a domain removes its tools;
- enabling a loaded typed domain adds its tools;
- reset reapplies the default enabled state;
- the server emits `notifications/tools/list_changed`.

New and queued calls to a disabled domain fail with `DOMAIN_DISABLED`. An
already active invocation may complete or be cancelled.

The server does not watch plugin directories. Adding, replacing, or removing a
plugin library requires restarting `ah mcp serve`.

`plugins list` adds MCP exposure information without renaming existing fields:

- whether the plugin is MCP-exposed;
- an omission reason when it is CLI-only.

## Migration Strategy

The typed core is introduced incrementally by domain, but MCP v1 is considered
complete only after every current bundled command is migrated.

For each command:

1. Define typed input and output DTOs.
2. Move domain logic out of CLI output code.
3. Implement the typed handler.
4. Route the CLI parser through the handler.
5. Add human-readable rendering.
6. Add descriptor, schemas, effects, and examples.
7. Add CLI and MCP contract tests.

No production schema-to-argv bridge is added. A command remains CLI-only until
its typed handler is complete.

Bundled dynamic plugins are part of the v1 migration:

- `github`
- `gitlab`
- `ollama`
- `postgres`

Their old and typed entry points call the same domain implementation.

Legacy third-party plugins remain CLI-only until updated by their authors.

## Testing

### Catalog Contract Tests

- MCP names are valid and unique.
- Every bundled command except `mcp.serve` has a descriptor.
- Every tool has input and output schemas.
- Every tool has complete effect and impact metadata.
- Tool ordering is deterministic.

### Schema Tests

- Schemas are valid JSON Schema 2020-12.
- Unknown input fields are rejected.
- Defaults and required fields are deterministic.
- Valid output conforms to `outputSchema`.
- Invalid plugin output produces `OUTPUT_SCHEMA_VIOLATION`.

### CLI and MCP Parity Tests

- The same typed input produces the same JSON payload through CLI and MCP.
- Released JSON field names and nesting remain unchanged.
- Success and failure paths are covered for every changed command.
- Text rendering is tested separately from command execution.

### Plugin Compatibility Tests

- API `1.0` plugins remain CLI-compatible and are omitted from MCP.
- Complete API `1.1` typed plugins appear in `tools/list`.
- Missing symbols or capabilities fail deterministically.
- Invalid catalogs and schemas are rejected.
- Invoke and cancellation entry points are exercised concurrently.

### Stdio Integration Tests

- Initialize and initialized lifecycle.
- `tools/list`.
- `tools/call`.
- Unknown tool protocol error.
- Invalid input tool execution error.
- Successful structured output.
- Output schema violation.
- Bounded queue behavior.
- Queued cancellation.
- Active cancellation.
- Timeout during queue wait.
- Timeout during execution.
- Live plugin enable, disable, reset, and list change notification.
- Stdout contains no bytes outside valid MCP JSON-RPC messages.

The official MCP Inspector is used for stdio smoke testing. Repository-owned
integration tests remain authoritative because the current official
conformance runner primarily targets URL-based server mode.

### Workspace Validation

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
ah run check cargo build --release --locked
```

## Documentation

Implementation updates:

- `docs/reference/mcp.md`
- `docs/reference/README.md`
- relevant domain command references when input or output wording changes
- `docs/agents/README.md`
- an MCP setup and usage recipe under `docs/agents/recipes/`
- `docs/developers/architecture.md`
- `docs/developers/plugins.md`
- `docs/developers/contributing.md`
- root `README.md`
- `ah ai info`

Documentation includes launch examples for common stdio MCP clients and states
clearly that AIHelper exposes mutating tools and unrestricted process-visible
filesystem paths.

## Acceptance Criteria

- `ah mcp serve` completes MCP initialization over stdio.
- Every current bundled command except `mcp.serve` is exposed as a typed tool.
- Every tool has deterministic input and output schemas.
- Every successful tool call returns validated structured content.
- CLI JSON and MCP structured output are contract-equivalent.
- Every tool declares impact and risk metadata.
- Legacy API `1.0` plugins remain usable through CLI and absent from MCP.
- Plugin state changes update the live tool catalog.
- Cancellation and timeouts keep the stdio control path responsive.
- Stdout remains a clean MCP JSON-RPC stream.
- Workspace and release validation pass.

## Future Extensions

The following are intentionally deferred behind stable boundaries:

- bounded parallel executor;
- per-domain concurrency limits;
- MCP Tasks;
- task persistence and polling;
- out-of-process isolation for untrusted plugins;
- live plugin directory reload;
- richer client-specific risk presentation.

These extensions may change executor and transport orchestration, but must not
change stable command IDs, input schemas, output schemas, or the typed plugin
symbols introduced by this design.

## References

- [MCP tools specification](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
- [MCP stdio transport specification](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports)
- [Official MCP Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Official MCP Inspector](https://github.com/modelcontextprotocol/inspector)
