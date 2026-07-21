# MCP P1 Shutdown Fixes Design

## Scope

Fix the three P1 findings from the asynchronous MCP review without changing the
public MCP protocol or job semantics:

1. Bound Tokio runtime shutdown when blocking handlers ignore cancellation.
2. Close the executor when the stdio transport reaches EOF or terminates.
3. Cancel direct executions when their MCP request/session future is dropped.

## Design

Transport shutdown and physical worker shutdown have separate responsibilities.
The HTTP and stdio transports close executor admission and trigger logical
cancellation before returning. `execute_mcp_serve` then consumes the Tokio
runtime with `Runtime::shutdown_timeout` and a single five-second grace period.
This avoids an unbounded normal runtime drop and avoids stacking independent
five-second physical-drain waits in every transport.

`ActiveExecution` owns the executor reference as well as the request-to-execution
mapping. Dropping the guard calls the executor's idempotent cancellation method
before removing the mapping. Normal completion remains safe because cancelling
an already terminal execution is a no-op. Detached jobs are unaffected because
their execution lifetime is owned by the job registry, not by this direct-call
guard.

## Error and Shutdown Behavior

- New work is rejected after executor close.
- Logical cancellation is immediate when a transport or direct request ends.
- Physical blocking handlers may finish naturally for up to five seconds.
- After the deadline, the MCP command returns even if a blocking handler remains
  uncooperative.
- Existing command results and transport errors remain unchanged.

## Verification

Add regression coverage for:

- stdio EOF closing the executor;
- dropping a direct MCP execution cancelling its executor entry;
- runtime shutdown returning within the configured bound when a blocking handler
  ignores cancellation.

Run the focused MCP/runtime tests, then the repository formatting, workspace test,
and debug build checks.
