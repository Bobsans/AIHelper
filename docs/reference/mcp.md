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

The same loopback server hosts protected secret setup at `/secrets/setup`.
`ah secrets add|edit --open` first mints a ten-minute, single-use 256-bit
capability through the local server, prints the setup URL, and then attempts to
open the form. Automatic browser opening is optional, so the printed URL also
works on headless hosts. The CLI accepts only a returned URL on the configured
origin with the exact `/secrets/setup?capability=...` shape. GET and POST both
require the capability; only a successful POST consumes it, and the response is
redacted metadata. Capability query values and form bodies are not written to
AIHelper logs. See [`ah secrets`](secrets.md).

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

HTTP commands that resolve project-relative paths, and every `ah.job.start`
target, must provide a non-empty absolute `context.cwd`. Stateless commands
fall back to the server default: `ai.*`, `plugins.*`, `ollama.*`, PostgreSQL
commands without a relative tool path, and HTTP requests without file inputs.

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
durable service definition. The sibling `ah-mcp-service.exe` is required and
becomes the Task Scheduler action. Equal repeated installs are idempotent.

To register or reconcile the task without starting or stopping any process:

```text
ah mcp service install --no-start
```

Start the registered definition explicitly:

```text
ah mcp service start
```

The controller returns after readiness while Task Scheduler runs the thin,
windowless `ah-mcp-service.exe` launcher. It starts the sibling `ah.exe` in a
kill-on-close job without creating or attaching to a console, so closing the
launching console does not stop the service. Foreground `mcp serve` and stdio
transport keep their normal console lifecycle.

Task Scheduler accepting a run request is not success by itself. `start` waits
up to 15 seconds for readiness whose version, PID, process instance UUID, and
configuration identity all match durable state. An already ready exact instance
returns idempotent success. A different server answering on the port is never
adopted or stopped.

Stop the registered service safely:

```text
ah mcp service stop
ah mcp service stop --json
```

`stop` first proves the exact durable PID and process instance UUID through
readiness, then submits identity-aware control shutdown. It allows five seconds
for graceful completion within one 15-second total deadline. If the exact
process remains, AIHelper may stop only a revalidated owned Task Scheduler
instance whose Scheduler UUID and engine PID match durable state. A queued
process-free retry can be cancelled only by its exact Scheduler UUID while the
instance lease is held free. Foreign endpoints, drifted execution properties,
PID mismatches, and ambiguous instances are never stopped. Successful actions
are `already_stopped`, `stopped`, and `forced_stopped`.

Restart under one lifecycle operation:

```text
ah mcp service restart
```

`restart` retains the stop proof while it revalidates the owned registration,
releases that proof immediately before Task Scheduler submission, and waits for
exact readiness from a new process UUID. It returns `restarted` for a previously
active service and `started` for a stopped service. A failure after stop leaves
the registration installed and retryable; it does not attempt to resurrect the
old process.

Uninstall the managed registration and semantic lifecycle metadata:

```text
ah mcp service uninstall
ah mcp service uninstall --json
```

`uninstall` is idempotent. It stops the exact managed instance, conditionally
deletes only the registration whose source, URI, and ownership marker still
match, then removes `runtime.json`, verified same-service definitions,
`current.json`, and `lifecycle.json`. A missing task permits exact orphan control
shutdown but never Scheduler fallback. The command preserves `ah.exe`, plugin
DLLs, configuration, logs, unexpected or external files, and the permanent
`lifecycle.lock` and `instance.lock` anchors. Successful actions are
`uninstalled` and `already_uninstalled`.

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

Install, start, stop, and restart use the same deterministic mutation schema
version 1. It always contains `command`, `schema_version`, `changed`, `action`,
`service_id`, `configuration_id`, `task_path`, `endpoint`, `registration`, and
`runtime`. Uninstall uses a separate schema with the same field order and emits
explicit `null` for unavailable last-known service ID, configuration ID, or
endpoint. `--quiet` suppresses successful text and JSON output; diagnostics are
still reported through the normal error channel.

The per-user task is stored at the Task Scheduler root as
`AIHelper Managed MCP - <CURRENT_USER_SID>`. AIHelper uses the typed Task
Scheduler COM API and refuses to overwrite a task without its exact ownership
marker. Canonical settings include one current-user logon trigger, one Exec
action, `IgnoreNew` multiple-instance policy, native Scheduler retries disabled,
no execution time limit, and battery operation enabled. Status compares
properties semantically and reports property-level drift; it does not compare
exported task XML.

The launcher performs at most three retries after the initial child launch,
with one-minute spacing. Keeping retry ownership in the launcher avoids relying
on Task Scheduler retry behavior and prevents duplicate retry loops. Fatal
managed startup or runtime failures persist a nonzero exit, while clean control
shutdown persists exit `0`. A `restart_backoff` runtime status requires the
launcher task to remain running, a durable nonzero managed failure, canonical
task settings, and no live readiness or instance lease. Status does not expose
the current attempt, remaining attempt count, or next retry timestamp.

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
`current.json` is a repairable index. Named Win32 mutex handles enforce one
lifecycle mutation and one managed server instance per user.

Important managed-service diagnostics include:

- `MCP_SERVICE_NOT_INSTALLED`
- `MCP_SERVICE_BUSY`
- `MCP_SERVICE_TASK_CONFLICT`
- `MCP_SERVICE_INSTALLATION_CONFLICT`
- `MCP_SERVICE_CONFIGURATION_DRIFT`
- `MCP_SERVICE_STATE_INVALID`
- `MCP_SERVICE_START_TIMEOUT`
- `MCP_SERVICE_WORKER_MISSING`
- `MCP_SERVICE_IDENTITY_MISMATCH`
- `MCP_SERVICE_SCHEDULER_FAILED`
- `MCP_SERVICE_RESTART_REQUIRED`
- `MCP_SERVICE_STOP_UNSAFE`
- `MCP_SERVICE_STOP_TIMEOUT`
- `MCP_SERVICE_TASK_CHANGED`
- `MCP_SERVICE_RESTART_FAILED`
- `MCP_SERVICE_UNINSTALL_INCOMPLETE`

Managed lifecycle commands are currently Windows-only. Manual stdio and HTTP
serve remain available on every supported platform.

## Tool names

Typed commands use:

```text
ah.<domain>.<command>
```

Examples include `ah.file.read`, `ah.search.text`, `ah.run.check`, and
`ah.plugins.disable`. `ah.mcp.serve` is intentionally not published because it
would recursively start another server.

When a descriptor declares credential slots, the generated tool description
lists each slot's accepted secret kinds. If an ID is unknown, follow that
description and call `ah.secrets.list` with its `kind` filter. `tools/list`
contains descriptor guidance only, never live vault record IDs. Calls pass IDs
under `credentials` (for example `credentials.database` for PostgreSQL or
`credentials.basic` for HTTP Basic); `/mcp` never accepts or returns the secret
value itself.

The PostgreSQL `--password-env` option is retained for direct CLI compatibility.
It is not an HTTP MCP credential channel; HTTP MCP callers use
`credentials.database` so the server resolves the value internally.

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
