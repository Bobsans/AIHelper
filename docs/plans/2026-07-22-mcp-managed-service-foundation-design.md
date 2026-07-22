# MCP Managed Service Foundation

## Scope

Implement the first coherent block of the managed HTTP MCP lifecycle on
Windows:

- a Task Scheduler 2.0 adapter that uses COM directly;
- a shared per-user lifecycle and future-upgrade lock primitive;
- versioned service definitions and durable runtime state;
- `ah mcp service install`, including `--no-start`;
- `ah mcp service start`;
- `ah mcp service status`;
- managed-process single-instance enforcement;
- bounded Task Scheduler restart settings;
- semantic task drift detection;
- stable scheduler and lifecycle diagnostics.

`stop`, `restart`, `uninstall`, updater execution, and the Windows VM matrix are
outside this block. Manual Claude Code, Codex, and OpenCode compatibility
testing also remains deferred.

## Considered architectures

### Put Windows orchestration directly in the CLI commands

This minimizes the initial number of types, but couples command routing,
Task Scheduler COM calls, durable schemas, locking, readiness polling, and
status reduction. It would make the next lifecycle commands and updater work
difficult to test independently.

### Use a lifecycle core with a Windows adapter

Keep lifecycle orchestration and state reduction platform-independent. Hide
Task Scheduler behind a small typed adapter and use fake implementations for
the main behavioral test matrix. This is the selected approach.

### Add a separate broker or helper process now

A broker would isolate Windows integration and simplify some future updater
handoff operations, but it would add another shipped executable, IPC contract,
and update surface before they are required.

## Module boundaries

Add a managed service module with responsibilities equivalent to:

```text
src/mcp_service/
  mod.rs
  lifecycle.rs
  lock.rs
  model.rs
  paths.rs
  readiness.rs
  runner.rs
  scheduler.rs
  windows.rs
```

The exact file split may be adjusted during implementation, but the boundaries
remain:

- `lifecycle` implements install, start, and status orchestration;
- `scheduler` owns canonical desired and observed task models and the adapter
  trait;
- `windows` is the only module that knows Task Scheduler COM interfaces;
- `model` owns versioned on-disk and output schemas;
- `lock` owns the per-user operation and instance leases;
- `runner` prepares managed server startup before plugin discovery;
- `readiness` probes and validates the local lifecycle endpoints.

The core accepts scheduler, readiness, clock, and identity providers where
deterministic tests require them. Filesystem behavior uses explicit injected
paths rather than a broad virtual filesystem abstraction.

## Windows bindings

Keep `windows-sys` for existing low-level Win32 calls. Add the Windows-only
`windows` crate for generated Task Scheduler, COM, OLE, and variant bindings.
The current `windows-sys` release does not project the Task Scheduler COM
interfaces, and manually maintaining vtables, `BSTR`, `VARIANT`, HRESULT, and
reference-counting code would create unnecessary unsafe surface.

Construct an `ITaskDefinition` through typed COM properties. Do not generate or
compare Task Scheduler XML. Scheduler normalization and implicit defaults make
raw XML equality unsuitable for configuration drift.

The adapter exposes operations equivalent to:

```text
inspect(task_identity) -> TaskObservation
register(desired_task) -> TaskObservation
run(task_identity) -> SchedulerRunReceipt
```

Later blocks extend the same adapter with stop and delete operations. Adapter
errors retain a stable AIHelper diagnostic code, the failed operation, and the
numeric HRESULT. Localized Windows messages are optional human detail and are
not part of JSON contracts or assertions.

## Task identity and ownership

Use the Task Scheduler root folder and a per-user task name:

```text
folder: \
name:   AIHelper Managed MCP - <CURRENT_USER_SID>
```

Including the current user's SID avoids collisions between users in the shared
Task Scheduler namespace without requiring AIHelper to create and secure a
custom task folder.

The task principal and logon trigger both target the same current-user SID.
Registration uses an interactive token, least privilege, and no stored
password.

Task ownership requires all of the following markers:

- `RegistrationInfo.Source` equals `AIHelper.ManagedMcp`;
- `RegistrationInfo.URI` uses
  `urn:aihelper:managed-mcp:v1:<SERVICE_ID>`;
- task data contains a compact versioned marker with owner, kind, service ID,
  configuration ID, and definition path.

If a task at the expected path lacks valid ownership markers, install returns
`MCP_SERVICE_TASK_CONFLICT` and does not overwrite it. Status reports the task
as configuration drift with an ownership reason.

## Canonical task definition

The expected task has exactly one current-user logon trigger and one Exec
action. The action is:

```text
Path:             <absolute ah.exe>
Arguments:        mcp serve --transport http --managed-config "<definition>"
WorkingDirectory: <absolute configured cwd>
```

`--managed-config` is hidden from normal help. It is recognized during raw-argv
preflight so the definition can set the config context, working directory,
runtime paths, and instance lease before dynamic plugin discovery.

The canonical settings are:

- current-user interactive token;
- least-privilege run level;
- enabled logon trigger for the current user;
- demand start enabled;
- `StartWhenAvailable = true`;
- `MultipleInstances = IgnoreNew`;
- `ExecutionTimeLimit = PT0S`;
- starting and continuing on battery are allowed;
- idle and network availability are not prerequisites;
- three restart attempts with `RestartInterval = PT1M`;
- task enabled.

The Windows adapter maps the registered task back into an `ObservedTaskSpec`.
Drift is the sorted semantic difference between desired and observed values.
Install reads the task back after registration and fails if required drift
remains.

## Machine-local paths

Resolve the base directory through the Windows Local AppData known-folder API:

```text
%LOCALAPPDATA%\AIHelper\managed-mcp\
  current.json
  definitions\<CONFIGURATION_ID>.json
  runtime.json
  lifecycle.json
  lifecycle.lock
  instance.lock
```

Do not derive this base from `AH_CONFIG_DIR`; lifecycle and future upgrade
commands must converge on one per-user lock even when command configuration is
overridden. Tests inject a complete temporary `ServicePaths` value.

The existing resolved AIHelper configuration directory remains supported. Its
absolute value is stored in the service definition and applied by managed
startup.

All persisted Windows paths must be absolute and valid Unicode. A path that
cannot be represented losslessly in the JSON and COM contracts is rejected with
`MCP_SERVICE_PATH_INVALID` rather than being stored through a lossy conversion.

## Durable service definitions

Do not make a single mutable service file the Task action source. Use immutable
versioned definitions plus an atomic current pointer.

`current.json` schema version 1 contains:

```json
{
  "schema_version": 1,
  "service_id": "uuid",
  "configuration_id": "uuid",
  "definition_path": "absolute path",
  "task_path": "\\AIHelper Managed MCP - <SID>"
}
```

A definition schema version 1 contains:

```json
{
  "schema_version": 1,
  "task_spec_version": 1,
  "service_id": "uuid",
  "configuration_id": "uuid",
  "user_sid": "SID",
  "executable_path": "absolute path",
  "working_directory": "absolute path",
  "config_directory": "absolute path",
  "runtime_state_path": "absolute path",
  "instance_lock_path": "absolute path",
  "expected_version": "1.1.0",
  "endpoint": {
    "host": "127.0.0.1",
    "port": 8787,
    "mcp_url": "http://127.0.0.1:8787/mcp",
    "readiness_url": "http://127.0.0.1:8787/health/ready"
  },
  "server": {
    "limit": null,
    "max_active": 32,
    "default_timeout_ms": 300000
  }
}
```

The service ID identifies one per-user managed registration. Idempotent install
from the same executable path reuses it. Semantic configuration equality also
reuses the configuration ID and definition. A supported semantic change creates
a new configuration ID and immutable definition.

An existing registration that resolves to another executable path is not
silently adopted. Install returns `MCP_SERVICE_INSTALLATION_CONFLICT`. A later
explicit uninstall and install may replace that registration. This keeps
service identity separate from the future updater installation identity.

Unknown schema versions and corrupt current or definition files return
`MCP_SERVICE_STATE_INVALID`. They are not overwritten through best-effort
recovery.

Install writes a new definition before task registration. After registration
and semantic readback succeed, it atomically writes `current.json`. A crash may
leave an unreferenced definition, but never a partially written definition.
Retry can recover the registered definition from the owned task marker and
action before rebuilding the current pointer. Cleanup retains the current
definition and any definition referenced by a running instance.

## Per-user leases

The existing create/delete sidecar persistence lock is not suitable for
long-lived lifecycle ownership. File age cannot prove that a process is dead,
and the updater will need a no-gap handoff.

On Windows, open `lifecycle.lock` and `instance.lock` through `CreateFileW` with
zero share mode. The open handle is the lease. The file remains on disk after
release; no stale-file deletion is involved.

This choice permits a future updater helper to inherit or receive a duplicated
handle to the same file object. The original and duplicate may overlap, so the
exclusive sharing restriction remains active while ownership moves between
processes.

Mutating lifecycle commands wait for the lifecycle lease for a bounded period.
If it remains occupied, they return `MCP_SERVICE_BUSY` with available operation
metadata. Status never waits. It performs one nonblocking observation and reads
diagnostic metadata when another operation owns the lease.

`lifecycle.json` schema version 1 stores the last operation and, while the lease
is held, its active state:

```text
operation_id
operation: install | start | stop | restart | uninstall | upgrade | rollback
pid
service_id
started_at
state: active | completed | failed
diagnostic_code
finished_at
```

The JSON is informative only. An active marker without an occupied lease is
stale history, not proof of a running operation.

The managed HTTP process holds `instance.lock` for its whole lifetime. A second
managed runner that cannot acquire the lease exits successfully without loading
plugins or starting HTTP. A zero exit avoids triggering a scheduler crash loop.
Task Scheduler `IgnoreNew` is defense in depth rather than the sole
single-instance mechanism.

## Managed runtime state

Managed argv preflight loads and validates the immutable definition, applies
its working and configuration directories, then acquires the instance lease.
Only after those steps may normal plugin discovery begin.

The runner creates a new process instance ID for each start and writes
`runtime.json` atomically. Runtime schema version 1 contains:

```text
schema_version
service_id
configuration_id
phase: starting | ready | stopping | stopped | failed
pid
version
instance_id
endpoint
started_at
updated_at
last_exit_kind
last_exit_code
diagnostic_code
```

Extend the HTTP serve path with a compatible managed entry point that accepts a
preselected instance ID and a listener-bound notification. Existing public
serve functions keep their current signatures and behavior.

The runner writes:

1. `starting` after acquiring the instance lease;
2. `ready` after the loopback listener is bound and the router, plugin catalog,
   and executor are available;
3. `stopping` when a managed control operation begins shutdown, when known;
4. `stopped` after a controlled successful exit;
5. `failed` after a handled startup or runtime failure.

Forced termination may leave an apparently active state. Status never treats a
runtime file as liveness proof without matching scheduler and readiness
evidence.

## Readiness validation

The readiness client uses short bounded requests to the loopback endpoint. A
ready managed instance is proven by all of the following:

- runtime service and configuration IDs match the desired definition;
- readiness version matches the definition's expected version;
- readiness PID matches runtime PID;
- readiness instance ID matches runtime instance ID;
- the endpoint and Host validation correspond to the desired loopback port.

An answering HTTP server with mismatched identity yields
`MCP_SERVICE_IDENTITY_MISMATCH`; it is never stopped or accepted as managed.

## Install behavior

Expose:

```text
ah mcp service install
  [--no-start]
  [--port <PORT>]
  [--max-active <N>]
  [--default-timeout-ms <MILLISECONDS>]
```

Global `--cwd` and `--limit` become part of the desired service definition.
The HTTP host remains fixed to `127.0.0.1`.

Install performs these steps under one lifecycle lease:

1. Resolve the per-user task identity and machine-local paths.
2. Load the current pointer, owned task, and referenced definition when present.
3. Reject ownership, installation, schema, and path conflicts without mutation.
4. Build the desired definition and canonical task spec.
5. Reuse an equal immutable definition or write a new one atomically.
6. Register or update the Task Scheduler task.
7. Read it back and require zero semantic drift.
8. Atomically publish `current.json`.
9. Remove only definitions that are neither current nor runtime-referenced.
10. If `--no-start` is absent, reconcile the exact desired ready instance.

If an old managed configuration is running and the server definition changed,
default install sends a control shutdown with the exact old instance identity,
waits for exit, then starts the newly registered definition. This block does
not yet provide Task Scheduler forced-stop fallback. If controlled replacement
cannot be proven safe, install returns `MCP_SERVICE_RESTART_REQUIRED` and does
not target an unknown process.

`--no-start` never stops an existing instance. It may therefore leave a valid
new registration alongside an old running configuration; status reports the
runtime configuration mismatch until a later restart.

Install is idempotent. Equal definition and task state return `changed=false`.
Owned task drift is repaired. An already ready exact instance is reused.

## Start behavior

Start performs these steps under one lifecycle lease:

1. Require a valid current definition and owned task.
2. Inspect the task and reject required configuration drift.
3. Return idempotent success for an exact ready instance.
4. If the expected process is already starting, wait without issuing another
   scheduler run.
5. Otherwise call Task Scheduler `RunEx`.
6. Wait up to one common 15-second deadline for a matching runtime record and
   readiness response.
7. On failure, refresh scheduler state, last result, runtime state, and
   readiness diagnostics before returning an error.

`RunEx` success proves only that the run request was submitted. Command success
requires exact HTTP readiness.

## Status behavior

Status is read-only. It does not register, repair, start, stop, or rewrite
state, and it never waits for the lifecycle lease. When the lease is free, it
may hold a successful nonblocking observation briefly while collecting a
consistent snapshot. When it is busy, the snapshot explicitly reports the
active operation.

JSON schema version 1 always contains these sections:

```json
{
  "command": "mcp.service.status",
  "schema_version": 1,
  "registration": {
    "status": "installed",
    "service_id": "uuid",
    "configuration_id": "uuid",
    "task_path": "task path",
    "definition_path": "absolute path"
  },
  "scheduler": {
    "state": "running",
    "last_result": 0,
    "last_result_hex": "0x00000000",
    "last_run_at": null,
    "diagnostic_code": null,
    "hresult": null
  },
  "runtime": {
    "status": "ready",
    "version": "1.1.0",
    "instance_id": "uuid",
    "pid": 1234,
    "endpoint": "http://127.0.0.1:8787/mcp",
    "started_at": "timestamp",
    "diagnostic_code": null
  },
  "readiness": {
    "status": "ready",
    "diagnostic_code": null
  },
  "lifecycle": {
    "status": "idle",
    "operation": null
  },
  "drift": []
}
```

Registration status is one of:

- `not_installed`;
- `installed`;
- `configuration_drift`;
- `scheduler_error`.

Runtime status is one of:

- `stopped`;
- `starting`;
- `running_not_ready`;
- `ready`;
- `stopping`;
- `restart_backoff`;
- `failed`;
- `identity_mismatch`.

The runtime reducer combines registration, scheduler, lifecycle lease, runtime
record, instance lease, and readiness observations. PID is considered reliable
only when runtime and readiness identity match.

Drift entries have stable field names and are sorted by field. Expected and
actual values are emitted only for non-secret properties. Status does not emit
credentials or environment contents.

`not_installed`, stopped, drifted, and scheduler-error snapshots are valid
status results with exit code zero. Automation inspects structured state. Only
failure to parse the command or serialize a snapshot is a command error.

## Mutation output

Successful install and start output use typed deterministic structures rather
than generic messages. They include:

```text
command
changed
action
service_id
configuration_id
task_path
endpoint
registration
runtime
```

Install actions are `installed`, `updated`, or `unchanged`. Start actions are
`started`, `waited`, or `already_ready`. Text output renders the same fields in
a fixed order. `--quiet` suppresses successful output but never errors.

## Stable diagnostics

The block defines at least these stable error codes:

```text
MCP_SERVICE_UNSUPPORTED_PLATFORM
MCP_SERVICE_NOT_INSTALLED
MCP_SERVICE_BUSY
MCP_SERVICE_TASK_CONFLICT
MCP_SERVICE_INSTALLATION_CONFLICT
MCP_SERVICE_CONFIGURATION_DRIFT
MCP_SERVICE_PATH_INVALID
MCP_SERVICE_STATE_INVALID
MCP_SERVICE_START_TIMEOUT
MCP_SERVICE_IDENTITY_MISMATCH
MCP_SERVICE_SCHEDULER_FAILED
MCP_SERVICE_RESTART_REQUIRED
```

Scheduler failures include the operation and numeric HRESULT. Start timeout
includes final scheduler, runtime, and readiness summaries without depending on
localized error text.

Non-Windows lifecycle commands return
`MCP_SERVICE_UNSUPPORTED_PLATFORM`. Existing manual `mcp serve` transports
remain cross-platform and unchanged.

## Tests

Add unit coverage for:

- schema version dispatch and rejection;
- semantic definition equality;
- canonical task-spec construction;
- deterministic semantic drift ordering;
- status reduction for every registration and runtime state;
- deterministic text and JSON output;
- Windows argument quoting and COM value mapping.

Add fake-adapter integration coverage for:

- new install and `--no-start`;
- idempotent reinstall;
- supported configuration update;
- owned drift repair and foreign-task conflict;
- not-installed and drifted start failures;
- already-ready and already-starting start behavior;
- `RunEx` followed by exact readiness;
- start timeout, scheduler failure, and identity mismatch;
- status snapshots for missing, stopped, starting, ready, stale, failed, and
  corrupt inputs;
- concurrent lifecycle commands;
- duplicate managed runners.

Add real filesystem and process coverage for exclusive-open lifecycle and
instance leases. Add Windows-only adapter contract tests for desired-to-observed
mapping. Real Task Scheduler mutation tests use a unique opt-in test identity
and a cleanup guard; normal workspace tests must not create persistent tasks.

The complete Windows 10, Windows Server 2016, logon-trigger, crash restart,
battery, and no-elevation matrix remains a separate roadmap item.

## Documentation and roadmap

Update:

- `docs/reference/mcp.md` with lifecycle commands, flags, outputs, status
  states, and diagnostics;
- `docs/agents/recipes/mcp-http.md` with managed install, start, and status
  workflows;
- generated or static AI command documentation that lists host commands.

After implementation and all applicable workspace checks pass, remove these
completed Stage 2 roadmap items:

```text
Реализовать Windows Task Scheduler 2.0 adapter...
Добавить общую per-user lifecycle- и upgrade-блокировку.
Определить durable runtime-state managed instance.
Реализовать ah mcp service install и --no-start.
Реализовать ah mcp service start.
Реализовать ah mcp service status.
Добавить single-instance enforcement.
Настроить ограниченный restart-on-failure и restart backoff.
Обнаруживать Task Scheduler configuration drift.
Сохранять полезную scheduler и lifecycle-диагностику.
```

Keep stop, restart, uninstall, and the combined lifecycle/Windows VM test item.
Keep target-client compatibility unchanged for later manual verification.

## Validation

Run focused tests while implementing, then:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

Also inspect the scoped diff, verify the Task Scheduler adapter compiles on
Windows, and confirm unrelated user changes remain unstaged and unmodified.

## Acceptance criteria

- A standard user can register an owned current-user task without elevation or
  a stored password.
- Install is idempotent, repairs owned semantic drift, and never overwrites a
  foreign task.
- Default install and start succeed only after exact HTTP readiness.
- `--no-start` never stops an existing process.
- Managed startup uses explicit immutable configuration before plugin
  discovery.
- Lifecycle and instance leases recover automatically after process death.
- Exactly one managed HTTP MCP process can run for the user.
- The task has bounded restart settings and no execution-time limit.
- Status separately reports registration, scheduler, runtime, readiness,
  lifecycle operation, and sorted drift.
- Text and JSON contracts are deterministic and contain stable diagnostics.
- Existing manual stdio and HTTP MCP behavior remains compatible.
- Completed roadmap entries are removed only after the full validation suite
  passes.
