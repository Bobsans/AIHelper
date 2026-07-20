# MCP Observability and Release Smoke Design

## Status

Accepted on 2026-07-20.

## Context

AIHelper MCP calls currently record only end-to-end `duration_ms`. When a
lightweight command expires behind a long-running command in the sequential
executor, the log makes the queued command look slow even though its handler
never started. Runtime unit tests cover queued timeout behavior, but the stdio
integration suite does not exercise that path.

Release archives also package dynamic plugins without executing the assembled
archive. Source tests can therefore pass while stale, missing, or incompatible
plugin binaries make a package incomplete.

## Goals

- Preserve the released executor, MCP event, plugin ABI, and JSONL contracts.
- Add accurate queue and execution timing to MCP command events when available.
- Distinguish queue timeout from execution timeout in logs.
- Cover head-of-line timeout through the real MCP stdio server.
- Execute every platform archive before it is uploaded.
- Verify packaged dynamic plugins through both CLI discovery and MCP tools.

## Non-Goals

- Replacing the sequential executor.
- Changing queue admission, timeout, cancellation, draining, or tombstone
  behavior.
- Starting command deadlines when handler execution begins.
- Supporting arbitrary root JSON Schema composition during MCP context
  injection.
- Invoking network- or credential-dependent plugin operations in release CI.

## Chosen Architecture

### Additive Executor Observation

The public `Executor::execute` method remains unchanged. The trait gains a
provided observed-execution method that delegates to `execute` and returns no
telemetry by default. Existing executor implementations therefore remain
source-compatible. Its output is an envelope containing both the unchanged
`Result<TypedInvocationResponse, RuntimeError>` and optional telemetry. The
telemetry therefore remains available when the result is an error, including
the queue timeout this feature is intended to explain.

`SequentialExecutor` overrides the observed path. After request validation it
records the existing deadline origin immediately before coordinator admission
checks. Telemetry is emitted only after a job is successfully enqueued, so
draining, duplicate-ID, queue-full, closed-worker, invalid-request, and timeout
overflow failures have no telemetry. The executor also records the instant at
which the coordinator changes an admitted request from queued to active.
Completion derives a bounded millisecond report without changing request state
transitions or error mapping.

The report contains:

- `queue_wait_ms`: deadline origin to activation, or deadline origin to a
  terminal event while the request is still queued;
- `execution_ms`: activation to completion or active timeout;
- `timeout_phase`: `queue` or `execution` only when the executor deadline wins.

For a queued timeout or queued cancellation, `queue_wait_ms` ends when the
timeout or cancellation wins and `execution_ms` is zero. Active cancellation
and handler errors include both known durations but no `timeout_phase`. Worker
failures include only timing that was observed before the failure. For
executors that do not provide observations, all new values are unavailable.

### Additive MCP Event Delivery

`McpCommandEvent` remains unchanged because adding public fields would break
external struct literals. `EventSink` gains a provided timing-aware method that
falls back to the existing `record_command` method. Existing sinks continue to
compile and receive one event per completed command.

The MCP adapter uses the observed executor path and carries optional timing
through its private call outcome. Errors before executor admission, including
unknown tools and invalid context, have no executor timing.

### JSONL Contract

`duration_ms` keeps its current boundary: the timer created in
`call_tool_completed` immediately before execution tracking begins, through
`call_tool_inner` and active-execution cleanup, ending immediately before event
delivery to the sink. It excludes sink I/O, catalog refresh/notification, and
JSON-RPC wire serialization. The logger adds optional top-level fields:

```json
{
  "duration_ms": 203,
  "queue_wait_ms": 201,
  "execution_ms": 0,
  "timeout_phase": "queue"
}
```

The executor timings do not need to sum exactly to `duration_ms` because MCP
adapter work and millisecond rounding occur outside the executor.

The fields are omitted when unavailable rather than emitted as `null`.
`schema_version` remains 1 because the extension is optional and additive.
Record compaction and minimal fallback preserve the fields when present.

## Release Archive Smoke Test

A standard-library Python script accepts the completed ZIP archive. It extracts
the archive into a fresh temporary directory and runs the extracted executable,
not the Cargo profile binary or pre-archive `dist` directory. This verifies the
same executable-relative plugin layout delivered to users.

Python ZIP extraction does not reliably restore POSIX mode bits. On Linux and
macOS the script explicitly adds the executable bit to the extracted `ah`
before launching it. Archive member validation still verifies that the binary
and platform plugin files are present in their expected relative locations.

The script uses an isolated `AH_CONFIG_DIR` and performs two checks:

1. `plugins list` JSON must report GitHub, GitLab, Ollama, and PostgreSQL as
   enabled dynamic plugins with MCP exposure.
2. A real MCP stdio handshake followed by `tools/list` must include one stable
   sentinel tool from each packaged dynamic plugin.

No plugin command is invoked, so the smoke test requires no credentials,
network service, Ollama server, or PostgreSQL client.

The release matrix runs the script after archive creation and before artifact
upload on Linux, Windows, and macOS. macOS becomes blocking rather than
`continue-on-error`; otherwise a broken macOS archive could still be published.

## MCP Head-of-Line Integration Test

The stdio process harness gains response correlation by JSON-RPC request ID and
reliable process cleanup. A new integration test:

1. starts `ah mcp serve` with bounded sequential execution and an isolated log;
2. sends a `run.check` child that creates a readiness marker and blocks;
3. waits for the marker to prove the first handler is active;
4. sends `file.stat` with a short timeout;
5. verifies that the second response reports `TIMEOUT` before the first ends;
6. releases the blocker and verifies the first response;
7. checks exactly one JSONL record for the timed-out request with
   `timeout_phase: "queue"`, positive `queue_wait_ms`, `execution_ms: 0`, and
   unchanged total `duration_ms` semantics.

Polling uses bounded deadlines rather than fixed sleeps. Timing assertions use
inequalities instead of exact millisecond values. A panic-safe release guard
creates the release marker during unwinding, and the blocker has its own bounded
maximum runtime. The MCP process guard then terminates its managed process group
and waits for exit, so a failed assertion cannot leave the blocking grandchild
running on either Unix or Windows.

## Error Handling

- Observation failures must not change command results or timeout behavior.
- Existing executors and event sinks that do not support telemetry omit fields.
- Release smoke failures stop the platform build before upload.
- Integration test cleanup terminates the MCP process and blocking child even
  after an assertion failure.

## Compatibility

- No existing JSON field changes meaning or becomes optional.
- No plugin C ABI type changes.
- No existing trait method is removed or changed.
- No public error variant changes.
- The MCP wire protocol and tool schemas remain unchanged.

## Documentation

Update the invocation logging reference with the optional timing fields and
timeout phase semantics. Update the MCP reference to clarify that observed queue
timeouts can now be distinguished in logs. Add an Unreleased changelog entry.

## Validation

- Unit tests for observed executor success, queued timeout, and active timeout.
- MCP adapter and event logger tests for optional timing propagation and
  compatibility fallback.
- Real stdio head-of-line integration test.
- Local invocation of the smoke script against a release ZIP where practical.
- `cargo fmt --all -- --check`.
- `cargo test --workspace --all-targets --locked`.
- `cargo build --locked`.
- `cargo build --release --locked`.
