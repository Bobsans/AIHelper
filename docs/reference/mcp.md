# `ah mcp`

AIHelper serves its typed command catalog through MCP over stdio or local
Streamable HTTP. Both transports use the same fail-fast parallel executor and
ordinary `ah.job.*` tools.

## `ah mcp serve`

```text
ah [--cwd PATH] [--limit N] mcp serve \
  [--transport <stdio|http>] \
  [--port PORT] \
  [--max-active N] \
  [--default-timeout-ms MILLISECONDS]
```

Defaults:

- `--transport stdio`
- `--port 8787` for HTTP
- `--max-active 32`
- `--default-timeout-ms 300000`

`--port` is valid only for HTTP. The removed `--max-queued` option fails with a
migration message pointing to `--max-active`; it is not reinterpreted.

`--json` is rejected because stdio reserves stdout for MCP messages and the HTTP
server is a long-running foreground command. Completed calls and transport
failures use the normal daily JSONL logs; see [Invocation Logging](logging.md).

## Transports

### Stdio

```text
ah --cwd D:\work\project --limit 200 mcp serve
```

The client owns this subprocess. A missing per-call `context.cwd` therefore
falls back to the server startup directory or global `--cwd`.

### Local Streamable HTTP

```text
ah mcp serve --transport http --port 8787
```

The endpoint is:

```text
http://127.0.0.1:8787/mcp
```

Readiness is available without creating an MCP session:

```text
GET http://127.0.0.1:8787/health/ready
```

It returns HTTP `200` with the exact running binary version, process ID, and a
UUID v4 identity that remains stable for the process lifetime:

```json
{
  "status": "ready",
  "version": "1.1.0",
  "pid": 1234,
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

A restarted process receives a new `instance_id`, including when the operating
system reuses its PID. Readiness uses the same Host and Origin restrictions as
the MCP endpoint and does not enable CORS.

The matching process can be stopped through the local control endpoint:

```text
POST http://127.0.0.1:8787/control/shutdown
Content-Type: application/json
```

The body must contain exactly the `instance_id` returned by the latest readiness
request:

```json
{
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

A matching request returns HTTP `202` before shutdown completes:

```json
{
  "status": "shutting_down",
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

An identity mismatch returns HTTP `409` with error code
`INSTANCE_ID_MISMATCH` and does not reveal the current identity. Invalid JSON,
an invalid UUID, missing or additional fields return `400`
`INVALID_SHUTDOWN_REQUEST`; a non-JSON content type returns `415`
`UNSUPPORTED_MEDIA_TYPE`. The control endpoint uses the same Host and Origin
restrictions as readiness and MCP, does not enable CORS, and has no additional
authentication. Read readiness immediately before shutdown so a restarted
process is not stopped by a stale controller.

One process can serve multiple stateful MCP sessions and repositories. The
plugin catalog, execution capacity, job registry, and retained results are
process-wide. Protocol request IDs and cancellation mappings remain
session-local.

HTTP execution has stricter context rules: every direct command and every
`ah.job.start` target must provide a non-empty absolute `context.cwd`. The daemon
startup directory is never an HTTP fallback.

The server binds only to `127.0.0.1`, validates `Host`, rejects nonlocal
`Origin`, and does not enable CORS. It has no authentication or TLS. Loopback is
not an authorization boundary: any local process running as the user can invoke
all published tools, including destructive tools.

## Managed Windows service

Windows users can register the HTTP server as a per-user Task Scheduler 2.0
task. Registration uses the current interactive user with least privilege; it
does not require elevation or store a password.

Install the canonical loopback service and wait for exact HTTP readiness:

```text
ah [--cwd PATH] [--limit N] mcp service install \
  [--port PORT] \
  [--max-active N] \
  [--default-timeout-ms MILLISECONDS]
```

The defaults match manual HTTP serve: port `8787`, `32` active handlers, and a
`300000` millisecond command timeout. `--cwd`, `--limit`, the resolved
configuration directory, and the absolute `ah.exe` path become part of the
durable service definition. Equal repeated installs are idempotent.

To register or reconcile the task without starting or stopping any process:

```text
ah mcp service install --no-start
```

Start the registered definition explicitly:

```text
ah mcp service start
```

Task Scheduler accepting a run request is not success by itself. `start` waits
up to 15 seconds for readiness whose version, PID, process instance UUID, and
configuration identity all match durable state. An already ready exact instance
returns idempotent success. A different server answering on the port is never
adopted or stopped.

Inspect every layer without repairing or starting the service:

```text
ah mcp service status
ah mcp service status --json
```

JSON status schema version 1 always includes `registration`, `scheduler`,
`runtime`, `readiness`, `lifecycle`, and sorted `drift` sections. Nullable fields
are emitted as explicit `null`. Not-installed, stopped, drifted, and scheduler
error snapshots are valid status results with exit code `0`; automation should
inspect the structured state and `diagnostic_code` fields.

The per-user task is stored at the Task Scheduler root as
`AIHelper Managed MCP - <CURRENT_USER_SID>`. AIHelper uses the typed Task
Scheduler COM API and refuses to overwrite a task without its exact ownership
marker. Canonical settings include one current-user logon trigger, one Exec
action, `IgnoreNew` multiple-instance policy, three restart attempts at a
one-minute interval, no execution time limit, and battery operation enabled.
Status compares properties semantically and reports property-level drift; it
does not compare exported task XML.

Durable machine-local state is independent of `AH_CONFIG_DIR`:

```text
%LOCALAPPDATA%\AIHelper\managed-mcp\
  current.json
  definitions\<configuration-id>.json
  runtime.json
  lifecycle.json
  lifecycle.lock
  instance.lock
```

Definitions are immutable. The registered task is activation authority and
`current.json` is a repairable index. Open Win32 handles with zero sharing
enforce one lifecycle mutation and one managed server instance per user; lock
files are never treated as stale merely because of their age.

Important managed-service diagnostics include:

- `MCP_SERVICE_NOT_INSTALLED`
- `MCP_SERVICE_BUSY`
- `MCP_SERVICE_TASK_CONFLICT`
- `MCP_SERVICE_INSTALLATION_CONFLICT`
- `MCP_SERVICE_CONFIGURATION_DRIFT`
- `MCP_SERVICE_STATE_INVALID`
- `MCP_SERVICE_START_TIMEOUT`
- `MCP_SERVICE_IDENTITY_MISMATCH`
- `MCP_SERVICE_SCHEDULER_FAILED`
- `MCP_SERVICE_RESTART_REQUIRED`

Managed lifecycle commands are currently Windows-only. Manual stdio and HTTP
serve remain available on every supported platform. Managed `stop`, `restart`,
and `uninstall` commands are planned but are not part of this command slice.

## Tool names

Typed commands use:

```text
ah.<domain>.<command>
```

Examples include `ah.file.read`, `ah.search.text`, `ah.run.check`, and
`ah.plugins.disable`. `ah.mcp.serve` is intentionally not published because it
would recursively start another server.

The adapter also publishes four transport-independent job tools:

- `ah.job.start`
- `ah.job.status`
- `ah.job.result`
- `ah.job.cancel`

The complete `ah.job.*` namespace is reserved for these built-in tools. Plugin
catalogs that publish a `job.*` command are rejected when the MCP server starts.

MCP protocol Tasks remain explicitly unsupported (`taskSupport: forbidden`) for
compatibility with Claude Code, Codex, and OpenCode. AIHelper jobs are ordinary
MCP tools and do not require Tasks capability.

## Starting and reading a job

`ah.job.start` accepts an exact published tool name and its normal arguments:

```json
{
  "tool": "ah.run.check",
  "arguments": {
    "command": ["cargo", "test"],
    "context": {
      "cwd": "D:\\work\\project",
      "timeout_ms": 600000
    }
  }
}
```

It validates and admits the target before returning:

```json
{
  "job_id": "job-a82f3c9100000000-0000000000000001",
  "tool": "ah.run.check",
  "status": "running",
  "draining": false
}
```

Pass `job_id` to the other job tools. `ah.job.result` never waits: it returns
`ready: false` while running and `ready: true` plus the retained typed response
after terminal completion. Results are repeatable and are not consumed when
read. Public states are `running`, `succeeded`, `failed`, `cancelled`, and
`timed_out`.

Cancellation and timeout become caller-visible immediately. If a plugin ignores
cooperative cancellation, the job reports its terminal state with
`draining: true` until the physical handler exits.

Jobs exist only in server memory. Physically completed records expire after one
hour. At most 128 records are retained; the oldest physically completed record
is evicted first, while running and draining records are never evicted.

## Parallel admission

Direct calls and job targets share `--max-active` physical execution slots. An
accepted command starts independently of every other command, including the same
tool in the same working directory. AIHelper has no execution queue and provides
no ordering, fairness, per-workspace lock, or per-plugin concurrency lane.

When all slots are held, a new target fails immediately with retryable
`EXECUTION_CAPACITY_FULL`; it is never scheduled for later. A cancelled or
timed-out uncooperative handler retains only its own slot while draining. Other
commands continue whenever another slot is available.

`queue_wait_ms` remains present for telemetry compatibility and is always zero.
A timeout phase is always `execution`.

## Per-call context

Every execution tool includes the reserved `context` object:

```json
{
  "context": {
    "cwd": "D:\\work\\project",
    "limit": 100,
    "timeout_ms": 30000
  }
}
```

- `cwd` resolves relative paths and sets child-process working directories.
- `limit` caps supported line or item collections.
- `timeout_ms` bounds command execution.

Commands must not change the process-global working directory. Calls using the
same `cwd` may overlap.

## Safety metadata and errors

Tools publish standard MCP annotations and `_meta["dev.aihelper/risk"]` with
`level`, `impact`, `effects`, and `reversibility`. Because `ah.job.start` can
dispatch any command, it is conservatively marked critical, destructive,
open-world, non-idempotent, and not read-only. Inspect the target tool before
starting it.

Successful direct calls return validated `structuredContent`, compact text, and
execution metadata. Operational failures return `isError=true` with a diagnostic
under `_meta["dev.aihelper/diagnostic"]`. Important control codes include:

- `EXECUTION_CAPACITY_FULL`
- `JOB_CAPACITY_FULL`
- `EXECUTOR_SHUTTING_DOWN`
- `JOB_NOT_FOUND`
- `INVALID_ARGUMENT`

Only protocol problems such as an unknown tool name use MCP protocol errors.

On stdio EOF, HTTP Ctrl-C, `SIGTERM` on Unix, or an accepted control shutdown,
admission closes immediately, sessions stop, and active work is cancelled. All
HTTP shutdown triggers enter the same idempotent lifecycle path. Protocol
draining and physical handler shutdown share one five-second budget; they do not
receive consecutive grace periods. In-memory jobs and results do not survive
restart.

Process exit codes distinguish clean lifecycle completion from fatal server
failure:

| Condition | Diagnostic | Exit code |
| --- | --- | --- |
| Lifecycle completes within the shared budget | none | `0` |
| Invalid startup configuration | configuration-specific code | `1` |
| Bind or unexpected transport failure | `MCP_SERVER_FAILED` | `1` |
| Shared shutdown budget expires | `MCP_SHUTDOWN_TIMEOUT` | `1` |

HTTP `202` means that a matching control request started shutdown; it does not
guarantee that draining will finish successfully. A supervisor should also
inspect the final process exit code and diagnostic logs.
