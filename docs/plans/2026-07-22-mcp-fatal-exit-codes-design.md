# MCP Fatal Exit Codes Design

Status: approved

Date: 2026-07-22

## Context

AIHelper already returns an error from MCP configuration, executor setup, HTTP
bind, and unexpected transport failures. `src/main.rs` converts those
`AppError` values into process exit code `1`.

The remaining gap is shutdown-budget expiry. Both bounded MCP transports wait
on the same `ShutdownTracker`, but their expiry branches currently return
`Ok(())`. A process that fails to complete graceful shutdown within the shared
five-second budget can therefore exit with code `0`.

## Goals

- Exit with code `0` after stdio EOF, Ctrl-C, SIGTERM, or control shutdown only
  when the transport completes within the shared shutdown budget.
- Exit with code `1` for invalid startup configuration, HTTP bind failure,
  unexpected transport failure, and observed shutdown-budget expiry.
- Give shutdown timeout a stable diagnostic code and deterministic message.
- Preserve the existing single shutdown deadline and remaining-budget handoff
  to event flushing and Tokio runtime shutdown.
- Preserve released JSON field names and the plugin ABI.

## Non-goals

- Add a configurable shutdown timeout.
- Add forced process termination or a second grace period.
- Redesign `McpServeOutcome` or its existing consuming methods.
- Complete the separate roadmap item for lifecycle tests with active jobs.
- Infer Tokio runtime timeout from elapsed wall-clock time; Tokio's
  `Runtime::shutdown_timeout` does not report whether all work finished.

## Decision

Add a typed `McpAdapterError::ShutdownTimeout { grace_ms }` variant. Its display
message uses an integer millisecond value and contains no platform-specific
debug formatting.

Both stdio and HTTP transport waits use one internal helper that races a mapped
transport future against `ShutdownTracker::expired()`. The transport-result
branch is listed first in a biased `tokio::select!`: if clean completion is
already observable in the same poll as the deadline, completion wins. A pending
transport whose tracker expires returns the typed timeout error.

The helper does not start or extend a timer. It uses the tracker deadline that
was started by the first shutdown trigger. Event flushing and
`Runtime::shutdown_timeout` continue receiving only the remaining duration.

`McpServeOutcome`, `into_parts()`, and `into_result()` retain their signatures.
Adding the error variant changes the Rust enum surface of the internal workspace
crate but does not change the stable plugin request/response ABI or MCP JSON
contracts.

## CLI and diagnostics

`execute_mcp_serve` maps errors as follows:

| Condition | Diagnostic code | Exit code |
| --- | --- | --- |
| Clean lifecycle completion within budget | none | `0` |
| Invalid CLI/startup configuration | existing code | `1` |
| HTTP bind or other transport failure | `MCP_SERVER_FAILED` | `1` |
| Shared shutdown deadline expires | `MCP_SHUTDOWN_TIMEOUT` | `1` |

The timeout continues through `record_mcp_system_error`, so the normal stderr
diagnostic and system event log both carry exit-code hint `1`.

## Data flow

1. EOF, Ctrl-C, SIGTERM, or `POST /control/shutdown` starts the existing
   `ShutdownTracker` deadline and closes executor admission.
2. The MCP transport begins graceful termination.
3. The shared wait helper observes either transport completion/failure or the
   existing deadline.
4. Clean completion returns success. Transport failure returns its existing
   adapter error. Deadline expiry returns `ShutdownTimeout`.
5. Event flushing and Tokio runtime shutdown use the remaining budget.
6. The root CLI prints any resulting diagnostic and exits with code `1`.

## Testing

Unit coverage in `ah-mcp` will verify:

- a pending transport and a short started tracker return the exact typed timeout;
- a ready clean transport completes successfully;
- an already-ready clean result wins a simultaneous deadline poll;
- an already-ready transport error is preserved rather than rewritten as a
  timeout.

Process integration coverage will verify:

- an occupied HTTP port exits quickly with code `1` and
  `MCP_SERVER_FAILED`;
- invalid MCP startup configuration exits with code `1` and the existing
  deterministic diagnostic;
- an incomplete HTTP request that keeps Axum graceful shutdown pending causes
  control shutdown to exceed the real five-second deadline, then exits with
  code `1` and `MCP_SHUTDOWN_TIMEOUT`;
- the existing successful control shutdown remains code `0`;
- stdio EOF remains code `0`;
- SIGTERM remains code `0` on Unix.

The incomplete-request test uses a loopback TCP connection with a declared body
larger than the bytes sent. It does not involve active jobs, so the later
active-job lifecycle roadmap item remains in scope.

## Documentation and roadmap

Update the MCP command reference and HTTP agent recipe with the exit-code
matrix. After all focused and workspace checks pass, remove only this completed
roadmap item:

```text
Гарантировать ненулевые exit codes для фатальных startup/runtime errors.
```

Keep the lifecycle-endpoint/active-job and client-compatibility items unchanged.

## Acceptance criteria

- No fatal MCP startup, bind, unexpected transport, or observed shutdown
  deadline failure exits with code `0`.
- Successful lifecycle triggers still exit with code `0`.
- Timeout diagnostics are stable in stderr and event logs.
- Both transports use the same timeout mapping and do not receive a new grace
  period.
- Focused tests and all required workspace checks pass.
