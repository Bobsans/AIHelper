# MCP Async Second-Review Fixes

## Status

Approved on 2026-07-21.

## Context

The fail-fast parallel MCP executor, local Streamable HTTP transport, and
process-local job tools remove the application execution queue. A second code
review found five remaining paths that can delay shutdown, starve admitted
commands, or leave the shared tool catalog stale:

- HTTP graceful shutdown is not bounded before runtime shutdown begins;
- stdio waits for RMCP draining before it closes executor admission;
- direct and background command logging can block Tokio execution resources;
- background catalog mutations are not published when the job completes;
- a stale notification task can remove a newer peer registration.

## Goals

- Keep shutdown within one five-second budget from the first shutdown signal or
  stdio EOF.
- Close admission and request cancellation as soon as shutdown begins.
- Keep command completion and MCP responses independent of event-sink latency.
- Preserve a hard bound on queued logging work and avoid Tokio's blocking pool.
- Publish catalog changes caused by detached jobs.
- Make stale-peer cleanup generation-safe.

## Non-Goals

- Changing the released MCP tool schemas or plugin ABI.
- Persisting jobs or log-dispatch state across restarts.
- Guaranteeing delivery of audit events when the bounded dispatcher is full or
  the process exits before pending events are written.
- Forcibly terminating arbitrary in-process plugin code.

## Design

### One shutdown budget

Shutdown uses a process-local tracker containing a once-set start instant and a
five-second grace duration. The first EOF, Ctrl-C, SIGTERM, or terminal transport
failure starts the tracker. Starting shutdown closes executor admission and
requests cancellation before waiting for protocol cleanup.

The stdio reader reports EOF independently of RMCP's `RunningService::waiting`
drain. The server closes the executor immediately, then lets RMCP drain only for
the remaining budget.

The HTTP server still uses Axum graceful shutdown, but the graceful server
future is abandoned when the shared deadline expires. Session cancellation and
executor closure begin when the signal arrives, not after Axum returns.

Transport functions return the shutdown timing to the runtime bootstrap. Tokio
runtime shutdown receives only the remaining budget. There is no second full
five-second grace period.

### Bounded event dispatcher

`McpShared` owns an optional event dispatcher instead of invoking `EventSink`
directly. The dispatcher has:

- a bounded non-blocking channel;
- one dedicated standard thread;
- panic isolation around each sink invocation;
- an atomic dropped-event counter.

Direct calls and jobs submit fully constructed events with `try_send`. They
never wait for queue capacity and never use Tokio's blocking pool. When the
queue is full or disconnected, the event is dropped and the counter increases.
This fail-open overload policy is intentional: command execution and protocol
responses must remain non-blocking.

Shutdown submits a flush barrier and awaits it only within the remaining shared
grace period, preserving preceding healthy events without delaying normal
responses. The worker thread is not joined because a broken sink may block
forever; process termination remains the final bound for a stuck sink.

### Job catalog completion hook

Every detached job receives a terminal completion hook backed by a weak
reference to shared MCP state. After the job stores its logical terminal result,
the hook compares the runtime catalog revision with the published snapshot. A
change rebuilds the snapshot once and broadcasts `tools/list_changed` to all
registered sessions.

The hook runs for every job; revision comparison makes the common no-change
path cheap. Failure to rebuild the catalog does not change the retained command
result and must not block physical lifecycle tracking.

### Peer generations

The shared peer map stores `{generation, peer}` rather than only a peer. Each
registration allocates a monotonically increasing generation. A detached
notification task removes a peer only when both session ID and generation still
match. A timeout from an older notification therefore cannot delete a newer
registration.

## Error Handling

- A transport drain deadline is a bounded shutdown outcome, not a reason to wait
  for another grace period.
- Event queue overflow increments the dropped counter and does not affect the
  command response.
- Event-sink panic is contained inside the dispatcher thread.
- Catalog refresh failure after job completion is best-effort and leaves the
  previous valid snapshot available.
- Executor cancellation remains cooperative; physical handlers may outlive the
  grace period until process termination.

## Testing

- Verify a blocking direct-call sink does not delay the MCP response.
- Verify enough blocking job sinks cannot starve a newly admitted command.
- Verify sink panic is contained and queue overflow increments the drop count.
- Run `plugins.enable` or `plugins.disable` through `ah.job.start` and verify the
  shared catalog generation and notification change at terminal completion.
- Verify an old notification timeout cannot remove a newer peer generation.
- Verify stdio EOF closes the executor before RMCP drain completes.
- Verify HTTP shutdown exits within the single grace period with an active
  connection and an uncooperative handler.
- Retain all existing parallel admission, job lifecycle, HTTP security, and
  cross-session tests.

## Validation

Run:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

The Unix SIGTERM integration remains platform-gated and must be reported when
validation runs only on Windows.
