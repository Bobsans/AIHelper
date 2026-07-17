# MCP Stdio Server Implementation Plan

## Objective

Implement the approved MCP stdio design in sequential, independently testable
slices. Preserve released CLI JSON and plugin ABI compatibility throughout the
migration.

## Delivery Order

1. Typed command and MCP foundation.
2. Host commands required by the long-lived server.
3. `ctx`.
4. `file`.
5. `git`.
6. `github`.
7. `gitlab`.
8. `http`.
9. `ollama`.
10. `postgres`.
11. `project`.
12. `run`.
13. `search`.
14. `task`.
15. Documentation, cross-domain tests, and release validation.

No command is exposed through MCP until its typed handler, schemas, effects,
CLI parity tests, and error tests are complete.

## Dependency Policy

- Pin `rmcp` to `2.2.0` with stdio server features only.
- Use Tokio only for the MCP I/O loop, bounded execution queue, deadlines, and
  cancellation coordination.
- Use `schemars` 1.x for built-in and bundled plugin DTO schemas.
- Use `jsonschema` without network or filesystem resolver features.
- Keep `ah-plugin-api` independent from `rmcp`, Tokio, and schema validators.
- Keep every new dependency in `Cargo.lock`.

## Phase 1: Typed Plugin Contracts

### Files

- `crates/ah-plugin-api/src/lib.rs`
- `crates/ah-plugin-api/Cargo.toml`

### Work

1. Raise plugin API minor version from `1.0` to `1.1`.
2. Add capability `typed_commands_v1`.
3. Add sidecar symbol constants and function pointer aliases:
   - `ah_plugin_command_catalog_json_v1`
   - `ah_plugin_invoke_command_json_v1`
   - `ah_plugin_cancel_command_v1`
4. Add protocol-independent data types:
   - `CommandCatalog`
   - `CommandDescriptor`
   - `CommandEffects`
   - `CommandEffect`
   - `RiskLevel`
   - `Reversibility`
   - `CommandExample`
   - `ExecutionContextWire`
   - `TypedInvocationRequest`
   - `TypedInvocationResponse`
   - `CommandNotice`
   - `CommandError`
5. Require descriptor schemas to be JSON objects.
6. Add stable constructors for success and failure responses.
7. Add C string conversion helpers for the new JSON contracts.
8. Add tests for serialization, defaults, compatibility, and error diagnostics.

### Validation

```text
ah run check cargo test -p ah-plugin-api
```

## Phase 2: Runtime Typed Catalog and Dynamic Loader

### Files

- `crates/ah-runtime/Cargo.toml`
- `crates/ah-runtime/src/lib.rs`
- new focused runtime modules under `crates/ah-runtime/src/`

### Work

1. Extend `BuiltinPlugin` with default CLI-only typed methods:
   - command catalog
   - typed invoke
   - cancellation
2. Load and validate optional dynamic typed symbols.
3. Reject partial `typed_commands_v1` implementations deterministically.
4. Aggregate enabled typed descriptors in stable command-ID order.
5. Validate:
   - unique command IDs;
   - domain ownership;
   - reserved `context` field;
   - JSON Schema validity;
   - complete effects and impact text.
6. Route typed calls without changing legacy `invoke`.
7. Validate successful output against `outputSchema`.
8. Add typed cancellation routing by request ID.
9. Add catalog generation/version tracking for list-change notifications.
10. Add compatibility fixtures for API 1.0, valid 1.1, and invalid partial
    typed plugins.

### Validation

```text
ah run check cargo test -p ah-runtime
```

## Phase 3: Sequential Executor

### Files

- new executor modules under `crates/ah-runtime/src/`

### Work

1. Add an `Executor` boundary independent from MCP.
2. Implement a bounded FIFO `SequentialExecutor`.
3. Track queued and active requests by stable request ID.
4. Start deadlines when requests are accepted.
5. Remove queued calls on cancellation.
6. Signal active built-in tokens and dynamic cancellation symbols.
7. Keep the I/O caller asynchronous while handlers use blocking domain code.
8. Return deterministic queue-full, timeout, cancellation, panic, and worker
   failure diagnostics.
9. Test that handlers never overlap in v1.
10. Test cancellation and deadline behavior without sleeping for long periods.

### Validation

```text
ah run check cargo test -p ah-runtime executor
```

## Phase 4: MCP Stdio Crate

### Files

- `Cargo.toml`
- `Cargo.lock`
- new `crates/ah-mcp/Cargo.toml`
- new `crates/ah-mcp/src/lib.rs`
- focused modules under `crates/ah-mcp/src/`

### Work

1. Add `ah-mcp` to the workspace.
2. Implement a dynamic `rmcp::ServerHandler`.
3. Declare tools capability with `listChanged`.
4. Map command descriptors to MCP tools:
   - `ah.` name prefix;
   - input and output schemas;
   - standard annotations;
   - `dev.aihelper/risk` metadata;
   - task support forbidden.
5. Extract the reserved `context` object from tool arguments.
6. Submit typed calls to `SequentialExecutor`.
7. Convert successful responses to validated structured content plus compact
   JSON text.
8. Convert command errors to `CallToolResult::error`.
9. Reserve protocol errors for unknown tools and malformed MCP requests.
10. Route client cancellation notifications.
11. Notify the client when the command catalog generation changes.
12. Ensure all server diagnostics use stderr and stdout remains transport-only.
13. Add an in-memory rmcp client/server integration test where practical.

### Validation

```text
ah run check cargo test -p ah-mcp
```

## Phase 5: Root Runtime and Host Commands

### Files

- `Cargo.toml`
- `src/cli.rs`
- `src/runtime_flow.rs`
- `src/lib.rs`
- `src/ai.rs`
- `src/plugins.rs`
- `src/plugin_settings.rs`
- new root command service modules
- `tests/integration/help.rs`
- `tests/integration/ai.rs`
- `tests/integration/plugins.rs`
- new MCP integration tests

### Work

1. Add `ah mcp serve`.
2. Reject `--json` for the server command.
3. Apply `--cwd`, `--limit`, `--quiet`, queue size, and default timeout.
4. Construct one shared runtime/catalog/executor for the MCP process.
5. Convert host commands to typed handlers:
   - `ai.info`
   - `plugins.list`
   - `plugins.enable`
   - `plugins.disable`
   - `plugins.reset`
6. Update live manager state after plugin mutations.
7. Increment catalog generation and emit list-change notifications.
8. Add MCP exposure fields to plugin list JSON as additive fields.
9. Verify CLI output remains unchanged except for approved additive JSON fields.
10. Add stdio purity and initialization tests.

### Validation

```text
ah run check cargo test --test integration ai
ah run check cargo test --test integration plugins
ah run check cargo test --test integration help
```

## Domain Migration Template

Apply the following sequence to every domain:

1. Inventory commands, inputs, outputs, global options, error codes, external
   effects, and current success/failure tests.
2. Define closed typed input DTOs.
3. Reuse or extract stable typed output DTOs with current JSON field names.
4. Generate input and output schemas.
5. Implement command descriptors and risk metadata.
6. Refactor domain execution to return typed data instead of printing.
7. Keep text rendering as a separate adapter.
8. Route legacy CLI invocation through the same typed handler.
9. Add MCP success, validation failure, execution failure, and parity tests.
10. Update domain reference and agent recipe documentation when behavior or
    discoverability changes.
11. Run the smallest domain-specific test before moving to the next domain.

## Domain Order and Scope

### `ctx`

- `ctx.pack`
- `ctx.symbols`
- `ctx.changed`

Primary concerns: byte limits, path arrays, truncation, and deterministic
symbol output.

### `file`

- `file.read`
- `file.head`
- `file.tail`
- `file.stat`
- `file.tree`

Primary concerns: unrestricted paths, explicit cwd resolution, symlink policy,
line ranges, and text/metadata output parity.

### `git`

- `git.status`
- `git.tags`
- `git.tag.create`
- `git.remotes`
- `git.changed`
- `git.diff`
- `git.blame`
- `git.commit-info`

Primary concerns: process cancellation, repository cwd, mutating tag effects,
and stable Git diagnostics.

### `github`

- repository detection
- issue list/view/create/update/close/comment/comments
- release get/assets/create
- workflow list/dispatch
- run list/get/wait/jobs/logs/warnings/artifacts

Primary concerns: credentials, open-world annotations, remote mutations,
pagination/limits, and long-running wait cancellation.

### `gitlab`

- project detection
- release list/get/create
- issue list/view/create/update/close/comment/comments
- pipeline list/get/wait/jobs
- job trace/warnings

Primary concerns mirror GitHub while preserving GitLab-specific JSON contracts.

### `http`

- `http.request`
- `http.get`
- `http.post`
- `http.replay`
- `http.assert`
- `http.run`

Primary concerns: arbitrary external access, secret redaction, nested request
payload schemas, assertion reports, timeout propagation, and mutation risk.

### `ollama`

- `ollama.ask`
- `ollama.chat`

Primary concerns: long response timeout, model/server errors, and structured
response metadata.

### `postgres`

- managed tool status/download/use/cleanup
- connection and metadata inspection
- read-only query and explain
- mutation/admin execution
- activity, locks, sizes, and settings

Primary concerns: password redaction, process cancellation, read-only versus
destructive classification, and large tabular output schemas.

### `project`

- `project.detect`
- `project.commands`
- `project.version`

Primary concerns: nested output DTOs and deterministic detection ordering.

### `run`

- `run.check`

Primary concerns: arbitrary process effects, argv arrays rather than shell
strings, stdout/stderr bounds, timeout, cancellation, and exit metadata.

### `search`

- `search.text`
- `search.files`

Primary concerns: repeated paths/globs, regex validation, limits, symlink
policy, and deterministic match ordering.

### `task`

- `task.save`
- `task.run`
- `task.list`

Primary concerns: persistent mutation, shell execution risk, cancellation, and
stable recipe storage/output.

## Cross-Domain Contract Tests

1. Every bundled command except `mcp.serve` appears exactly once.
2. Tool names remain deterministic and unique.
3. Every tool schema is an object and reserves `context`.
4. Every tool provides output schema and complete effects.
5. CLI JSON equals typed/MCP structured output for shared fixtures.
6. Plugin disable/enable updates the live tool list.
7. No command writes to stdout while running under MCP.
8. All error codes remain stable.

## Documentation

Update:

- `README.md`
- `docs/reference/README.md`
- new `docs/reference/mcp.md`
- every affected domain reference
- `docs/agents/README.md`
- new MCP setup recipe
- domain AI recipes where typed parameters need examples
- `docs/developers/architecture.md`
- `docs/developers/plugins.md`
- `docs/developers/contributing.md`
- `CHANGELOG.md`
- `ah ai info`

## Final Validation

Run after the last migrated domain:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
ah run check cargo build --release --locked
```

Also run MCP Inspector CLI smoke tests for:

- tools list;
- one read-only tool;
- one filesystem mutation tool;
- one process tool;
- one external network tool when credentials/services are available;
- cancellation of a long-running call.

## Completion Gate

The implementation is complete only when:

- all bundled commands have typed handlers and schemas;
- every domain passes CLI/MCP parity tests;
- bundled dynamic plugins expose valid typed sidecar ABI;
- legacy third-party plugins remain CLI-compatible;
- the server stays responsive to cancellation while a handler runs;
- stdout is a clean MCP stdio stream;
- all required documentation and workspace checks pass.
