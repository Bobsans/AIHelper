# MCP Parallel HTTP Jobs Design

**Date:** 2026-07-20  
**Status:** Approved for implementation

## Context

AIHelper currently exposes typed commands through one MCP stdio process backed
by a bounded sequential executor. A long-running command therefore delays every
later command handled by that process. MCP Tasks cannot solve this portably yet:
Claude Code, Codex, and OpenCode do not all enable the experimental Tasks
capability.

AIHelper will add a local Streamable HTTP transport so Claude Code, Codex, and
OpenCode can connect to one long-lived process. The same release will replace
MCP execution serialization with fail-fast parallel execution and add ordinary
MCP job tools for non-blocking result retrieval.

## Goals

- Run accepted MCP command invocations concurrently, including calls to the
  same command with the same working directory.
- Never place command execution in an AIHelper queue. Start immediately or
  return a capacity error immediately.
- Give stdio and Streamable HTTP the same parallel execution semantics.
- Let multiple local HTTP clients share one plugin catalog, executor, job
  registry, and telemetry stream.
- Let a client start a command without keeping its `tools/call` request open.
- Preserve typed command validation, cancellation, timeouts, risk metadata, and
  existing plugin ABI layout.
- Keep execution and retained-result memory bounded.

## Non-goals

- MCP Tasks support before all target clients can consume it reliably.
- LAN or remote network access.
- Authentication, OAuth, TLS, or browser CORS support.
- Persistence of jobs or results across process restarts.
- Execution ordering, per-client fairness, per-workspace locking, or
  per-plugin concurrency limits.
- Forced termination of arbitrary in-process plugin code.
- A background service manager with install, start, stop, PID-file, or autostart
  behavior. The HTTP server remains a foreground command.

## Chosen Approach

Use a stateful MCP Streamable HTTP service with a process-wide shared core and
one thin MCP handler per client session. Replace the MCP `SequentialExecutor`
with a fail-fast `ParallelExecutor`. Add adapter-native `ah.job.*` tools that
store process-local execution handles and results.

Three alternatives were rejected:

- Unbounded task or thread spawning can exhaust processes, threads, file
  descriptors, connections, and memory. It also eventually creates a hidden
  runtime queue.
- Stateless Streamable HTTP weakens request cancellation, notifications, and
  session recovery for no meaningful job API benefit.
- An HTTP-to-stdio gateway or process-per-command worker adds another protocol,
  repeats plugin startup, and breaks the single in-process runtime goal.

## Command-Line Interface

Existing stdio startup remains the default transport:

```text
ah mcp serve --max-active 32
```

The local HTTP server is started with:

```text
ah mcp serve --transport http --port 8787 --max-active 32
```

The HTTP endpoint is fixed at:

```text
http://127.0.0.1:8787/mcp
```

Rules:

- `--transport` accepts `stdio` and `http`; the default is `stdio`.
- `--port` defaults to `8787` and is valid only for HTTP.
- There is no `--host` or `--bind` option in this release.
- `--max-active` defaults to `32` and must be at least one.
- The removed `--max-queued` option fails with a migration diagnostic that
  points to `--max-active`; it is not silently reinterpreted.
- `--default-timeout-ms` keeps its current meaning.

## Process and Session Boundaries

The process-wide core owns:

- `PluginManager` and the typed command registry;
- the immutable catalog snapshot and catalog generation;
- one `ParallelExecutor` and its global capacity;
- one `JobRegistry`;
- globally unique execution and job ID generators;
- the event sink and telemetry counters;
- the active-session notification hub;
- shutdown state.

Each stateful HTTP session gets a separate MCP handler. Session-local state owns
the mapping from that session's JSON-RPC request IDs to globally unique
execution IDs and its notification peer. Different clients commonly reuse
small JSON-RPC IDs such as `1`; keeping this mapping session-local prevents one
client's cancellation notification from cancelling another client's command.

The HTTP transport uses rmcp's stateful `StreamableHttpService` and
`LocalSessionManager`. Deleting or losing a session does not cancel background
jobs. Jobs are visible to any connected local client that knows their job ID.

Stdio creates one handler over the same shared-core abstractions. It uses the
same parallel executor and job semantics as HTTP, but the core lives only for
that stdio subprocess.

## Working Directories

The HTTP process may serve clients working in different repositories. A daemon
startup directory is therefore not a safe implicit execution context.

- Every direct HTTP execution tool requires `context.cwd`.
- The target arguments supplied to HTTP `ah.job.start` require
  `context.cwd`.
- The value must be a non-empty absolute path.
- Missing or relative values fail before job reservation or executor
  admission.
- Commands with the same absolute `cwd` are allowed to overlap.
- The executor never derives lanes, locks, or identity from `cwd`.
- Stdio retains its existing optional `context.cwd` fallback because the client
  owns and launches that process.

MCP Roots may provide useful context in clients that support them, but Roots
are not required and are not an execution-authority or compatibility
dependency.

## Parallel Executor

`ParallelExecutor` has no worker queue and no queued state. It exposes an atomic
non-blocking submission operation that returns an `ExecutionHandle`.

```text
try_submit
  |-- capacity available --> running --> succeeded | failed
  |                                  \-> cancelled | timed_out
  \-- capacity exhausted --> EXECUTION_CAPACITY_FULL
```

Admission uses a process-wide capacity of 32 active physical executions.
Synchronous direct tools and targets submitted by `ah.job.start` share the same
capacity. Management calls such as `job.status`, `job.result`, `job.cancel`,
`tools/list`, and ping do not consume execution slots.

The executor uses non-blocking permit acquisition. When no permit is available,
it returns retryable `EXECUTION_CAPACITY_FULL` immediately. The invocation is
never scheduled to run later. An accepted synchronous call waits only for its
own result; it does not prevent other calls from being admitted or executed.

Typed handlers remain synchronous internally and run on blocking workers. The
runtime blocking-worker capacity must stay above `max-active`, so AIHelper does
not introduce a second application-level execution queue. Normal OS and runtime
scheduling latency is not treated as queued command execution.

## Logical Completion and Physical Draining

Logical result state is distinct from physical handler state.

- A normal completion records `succeeded` or `failed` and releases its permit.
- Cancellation and timeout complete the caller-facing result immediately and
  request cooperative plugin cancellation.
- A handler that ignores cancellation becomes physically draining while its
  logical result remains `cancelled` or `timed_out`.
- A draining handler retains exactly one execution permit until it actually
  exits.
- Late success or failure never overwrites an existing cancellation or timeout.
- Other executions continue while any capacity remains. There is no global
  draining gate.
- If all 32 permits are held by running or draining handlers, new execution is
  rejected immediately.

Existing telemetry retains `queue_wait_ms` with the deterministic value `0`.
Timeout phase is always execution. `execution_ms` measures time to logical
completion. Physical drain completion is recorded as a separate lifecycle
event rather than as a second command completion.

## Job Tools

The adapter publishes four ordinary MCP tools and continues to advertise
`TaskSupport::Forbidden`:

```text
ah.job.start
ah.job.status
ah.job.result
ah.job.cancel
```

`ah.job.start` accepts an exact published MCP tool name and the same argument
object used for a direct invocation:

```json
{
  "tool": "ah.run.check",
  "arguments": {
    "command": ["cargo", "test"],
    "context": {
      "cwd": "D:\\Work\\Project",
      "timeout_ms": 600000
    }
  }
}
```

It cannot target `ah.job.*` and never recurses through the CLI. The target is
resolved and invoked through the in-process typed registry.

Successful start returns only after the target has been validated, a job record
has been reserved, and executor admission has succeeded:

```json
{
  "job_id": "job-a82f3c91-00000001",
  "tool": "ah.run.check",
  "status": "running"
}
```

Public job states are:

```text
running | succeeded | failed | cancelled | timed_out
```

There is no public `queued` state. A `draining` boolean reports whether a
logically terminal cancellation or timeout still has a physically running
handler.

`job.status`, `job.result`, and `job.cancel` accept only `job_id`.
`job.result` never waits. It returns `ready: false` for a running job and a
stable result envelope for a logically terminal job. A target command failure
is data inside that envelope; it does not make the `job.result` control call an
MCP error. Control-plane failures such as `JOB_NOT_FOUND` remain MCP tool
errors. Results are repeatable and are not consumed when read.

Cancellation and timeout therefore return `ready: true` immediately, including
while `draining: true`; callers never need to wait for physical cleanup to
observe the terminal result.

`job.cancel` is idempotent. Cancelling the completed `job.start` request or
disconnecting its session does not cancel the job.

## Atomic Job Admission

Job startup uses a hidden reservation so clients never observe phantom jobs:

1. Resolve the target tool and validate its typed arguments and execution
   context.
2. Under the job-registry lock, remove expired records, evict eligible terminal
   records, allocate a boot-scoped ID, and reserve an unpublished record.
3. Release the registry lock and call the executor's non-blocking submission.
4. On submission failure, remove the reservation before returning the error.
5. On success, attach the self-contained execution handle and publish the job
   before returning its ID.

The execution handle retains state and completion independently, so a very fast
completion before attachment cannot lose the result. Registry and executor
locks are never held across an await or plugin callback.

## Job Identity, Retention, and Capacity

Job IDs combine a process boot nonce and a monotonic sequence. A stale ID held
by a client after restart therefore cannot address a new job accidentally.

- The registry stores at most 128 records.
- Cleanup runs lazily on job operations and completion; no sweeper is required.
- Logically and physically completed records expire one hour after physical
  completion.
- Under pressure, the oldest physically completed record is evicted first,
  ordered deterministically by completion time and sequence.
- Running and draining records are never evicted.
- If all 128 records are running or draining, start fails with retryable
  `JOB_CAPACITY_FULL`.
- A terminal logical result with `draining: true` remains active for retention
  and capacity purposes until the handler exits.

Stored responses remain subject to existing per-command output limits. The job
registry introduces neither unbounded stream buffering nor a second unbounded
copy of command output.

## Error Contract

Stable control-plane diagnostics include:

- `EXECUTION_CAPACITY_FULL`: all 32 execution permits are held; retryable.
- `JOB_CAPACITY_FULL`: all 128 job records are running or draining; retryable.
- `EXECUTOR_SHUTTING_DOWN`: new admission is closed; not retryable in this
  process.
- `JOB_NOT_FOUND`: the job is unknown, expired, or evicted.
- `INVALID_ARGUMENT`: unknown or recursive target, invalid schema, or invalid
  context.

Capacity failures never create retained job records and never start later.

## Plugin Concurrency Contract

The plugin ABI layout does not change. Existing Rust built-ins already implement
`Send + Sync`, and contributor documentation already requires handlers to
protect shared state in preparation for parallel execution.

The documented runtime contract becomes explicit:

- multiple invocations of the same built-in or dynamic plugin may overlap;
- cancellation may run concurrently with invocation;
- plugins must synchronize mutable caches, configuration, and cancellation
  registries;
- commands must use invocation `context.cwd` and child-process `current_dir`
  instead of changing process-global cwd;
- disabling a plugin prevents new invocation after the state change but does
  not unload code or forcibly terminate an already admitted invocation.

Bundled dynamic plugins and host commands require a concurrency audit and
stress tests. AIHelper guarantees concurrent admission and invocation, but it
does not guarantee that an external resource or a plugin's own internal lock is
contention-free.

## Catalog Changes and Notifications

The catalog snapshot and generation are process-wide. Enabling or disabling a
plugin in one HTTP session updates the shared catalog. A notification hub sends
best-effort `tools/list_changed` to every active session peer. Stale peers are
removed without failing the command that changed the catalog.

## HTTP Security

The first HTTP release is deliberately local-only:

- bind only to `127.0.0.1`;
- allow only the actual loopback authority in `Host`;
- allow requests without `Origin`, as expected from CLI clients;
- reject an invalid or nonlocal `Origin` with HTTP 403;
- do not enable CORS;
- apply bounded HTTP request-body and connection settings supplied by the HTTP
  stack;
- do not log authorization-like headers or unredacted arguments beyond the
  existing event policy.

There is no authentication. Documentation must state that loopback is not an
authorization boundary: any local process running as the user can invoke all
published tools, including destructive tools.

Because `ah.job.start` can dispatch a target whose risk is not expressible in
static annotations, the tool uses conservative worst-case annotations:
destructive, open-world, non-idempotent, not read-only, and critical-risk
metadata equivalent to the most capable published target. Its description
instructs clients to inspect the target tool's metadata before starting it and
does not promise that retries are safe.

## Shutdown

On Ctrl-C or process termination:

1. Atomically close executor and job admission.
2. Stop accepting HTTP connections and close MCP sessions.
3. Request cancellation of every active synchronous execution and job.
4. Wait a fixed five-second grace period for physical handlers to exit.
5. Terminate the process even when an in-process plugin ignored cancellation.

Job state and results are lost at process exit. Supported child-process commands
must continue to terminate their process groups through existing cooperative
cancellation paths.

## Observability

Each admitted target produces exactly one logical command completion event with
its execution ID, optional job ID, command, transport, status, duration, and
execution telemetry. Job control calls are recorded separately and do not
duplicate target completion events.

Additional lifecycle counters or events cover active executions, draining
executions, capacity rejections, job eviction, and physical drain completion.
Output remains deterministic in field names and ordering.

## Testing

### Runtime

- Prove 32 gated handlers can overlap and the 33rd fails immediately without
  starting later.
- Prove same-command, same-cwd, different-cwd, synchronous, and job executions
  overlap.
- Verify direct calls and jobs share one capacity pool.
- Verify panic, spawn failure, normal completion, cancellation, and timeout
  release permits exactly once.
- Verify ignored cancellation retains one permit, never activates a global
  drain, and cannot overwrite the logical result.
- Verify `queue_wait_ms` is always zero.

### Job Registry

- Cover validation failure, reservation rollback, completion before handle
  attachment, and concurrent starts.
- Cover idempotent cancellation and completion/cancel/timeout races.
- Cover one-hour TTL, deterministic oldest-terminal eviction, 128 active or
  draining records, repeatable results, and stale boot-scoped IDs.

### MCP and HTTP

- Exercise start, status, result, and cancel without client Tasks capability.
- Prove management tools remain responsive at full execution capacity.
- Create two HTTP sessions with the same JSON-RPC request ID and verify
  cancellation isolation.
- Start in one client session, disconnect it, and read or cancel from another.
- Reject missing or relative HTTP cwd before admission.
- Reject hostile Host and Origin values and any attempt to configure a
  non-loopback bind.
- Verify shared catalog refresh and `tools/list_changed` broadcast.
- Verify graceful shutdown releases the port and does not wait indefinitely for
  an uncooperative plugin.
- Retain stdio integration coverage with the new parallel semantics.

### Plugins and Documentation

- Stress concurrent invocation and cancellation for bundled built-in and
  dynamic plugins.
- Verify concurrent settings and task-store writes remain transactional.
- Update the MCP command reference, stdio recipe, HTTP connection examples for
  Claude Code, Codex, and OpenCode, plugin concurrency contract, and migration
  guidance from `--max-queued` to `--max-active`.

## Acceptance Criteria

- No accepted MCP command waits behind another AIHelper command.
- Up to 32 physical handlers run concurrently across direct calls and jobs.
- The 33rd execution fails immediately and never starts later.
- A timed-out or cancelled uncooperative handler consumes only its own slot.
- Multiple local HTTP clients share jobs and catalog state without sharing
  protocol request identity.
- All HTTP executions use an explicit absolute working directory.
- Existing typed response schemas and plugin ABI layout remain compatible.
- Both stdio and HTTP pass the parallel execution, cancellation, job lifecycle,
  security, and shutdown test suites.
