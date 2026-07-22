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

## Early command routing

All managed lifecycle paths must be classified before normal runtime startup.
The raw-argv pre-router recognizes these built-in forms without consulting the
plugin catalog:

```text
ah mcp service install ...
ah mcp service start ...
ah mcp service status ...
ah mcp serve --transport http --managed-config <PATH>
```

It understands global options, including `--cwd`, `--json`, `--quiet`, and
`--limit`, without treating their values as command tokens. The pre-router
either returns `NotManaged` or a fully parsed managed command; malformed
managed syntax is an early CLI error and must not fall through to plugin
discovery.

For `install`, `start`, and `status`, early execution may load `ConfigContext`,
the event logger, managed service state, and Windows APIs. It must not construct
the plugin manager, enumerate plugin directories, or load dynamic plugin DLLs.

For managed serve, preflight validates the immutable definition, applies its
explicit working and configuration directories, and acquires the instance
lease. Normal startup and plugin discovery begin only after that succeeds.
The event logger is created from the managed definition's resolved config
context, not from the caller's ambient environment.

Manual `mcp serve`, AI information, plugin commands, and dynamic domains keep
the existing startup and discovery flow. The early router therefore preserves
the current plugin-driven CLI while satisfying the requirement that lifecycle
and recovery run before dynamic DLL loading.

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

The task data schema is exact:

```json
{
  "schema_version": 1,
  "owner": "AIHelper",
  "kind": "managed_mcp",
  "service_id": "lowercase hyphenated UUID",
  "configuration_id": "lowercase hyphenated UUID",
  "definition_path": "absolute Windows path"
}
```

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

Executable, working-directory, config-directory, definition, and task-action
paths use one Windows identity policy:

1. Resolve relative input against the already applied command working
   directory.
2. Require an absolute path and canonicalize existing path components.
3. Normalize separator and trailing-separator differences.
4. Compare normalized UTF-16 paths with ordinal case-insensitive Windows
   comparison.

Do not expand environment variables in persisted or observed task paths. The
same comparison helper is used for idempotency, installation conflict checks,
task drift, and readiness configuration identity. Path display in text and JSON
retains the normalized absolute spelling selected during install.

All UUIDs in on-disk and command JSON are lowercase hyphenated strings. All
timestamps are UTC RFC 3339 strings with millisecond precision, for example
`2026-07-22T12:34:56.789Z`. PIDs are unsigned 32-bit integers. HRESULT values
are signed 32-bit integers plus uppercase eight-digit hexadecimal strings.

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

Every field shown in `current.json`, the task marker, and the definition is
required. `server.limit` is the only nullable definition field and is either
`null` or a positive platform `usize` representable by the running binary.
Ports are unsigned 16-bit integers greater than zero. Counts and timeouts are
positive integers. Schema readers reject unknown schema versions before
interpreting later fields.

The service ID identifies one per-user managed registration. Idempotent install
from the same executable path reuses it. Semantic configuration equality also
reuses the configuration ID and definition. A supported semantic change creates
a new configuration ID and immutable definition.

An existing registration that resolves to another executable path is not
silently adopted. Install returns `MCP_SERVICE_INSTALLATION_CONFLICT`. A later
explicit uninstall and install may replace that registration. This keeps
service identity separate from the future updater installation identity.

For mutating commands, an unknown schema version or malformed owned definition
returns `MCP_SERVICE_STATE_INVALID`. Such a file is not overwritten through
best-effort recovery. Read-only status instead returns a diagnostic snapshot:
the affected section uses `MCP_SERVICE_STATE_INVALID`, and drift identifies the
invalid component.

Immutable definitions are created with `create_new`, fully written, flushed,
closed, reopened, and deserialized before task registration. If the chosen UUID
path already exists, identical verified content is reusable; different or
invalid content returns `MCP_SERVICE_STATE_INVALID`. Immutable definitions are
never replaced in place. `current.json`, lifecycle metadata, and runtime state
continue to use atomic replacement.

Install writes and verifies a new definition before task registration. After
registration and semantic readback succeed, it atomically publishes
`current.json`. The registered owned task is the activation authority; the
current pointer is an index that can be rebuilt only from a valid owned marker
and valid immutable definition with the same service ID. Automatic rebuilding
is limited to a missing or well-formed stale pointer; malformed or newer-schema
pointers are never overwritten.

### Crash reconciliation matrix

Let `A` be the pointer's configuration and `B` the configuration referenced by
the registered task.

| Task observation | Current pointer | Definition | Status result | Mutating recovery |
| --- | --- | --- | --- | --- |
| absent | absent | none | `not_installed` | create a new service |
| absent | valid `A` | valid `A` | `not_installed`, drift `task.missing` | reuse the service ID only when executable identity matches, then register desired config |
| owned `A` | valid `A` | valid `A` | normal installed snapshot | normal reconciliation |
| owned `B` | missing | valid `B` | `configuration_drift`, `current.missing` | rebuild pointer to `B` |
| owned `B` | valid `A` with same service | valid `B` | `configuration_drift`, `current.configuration_id` | task wins; rebuild pointer to `B` |
| owned `B` | valid `B` | missing or invalid `B` | `configuration_drift`, state-invalid diagnostic | fail without overwrite |
| owned `B` | malformed pointer | valid `B` | `configuration_drift`, `current.invalid` | fail without overwriting an unreadable or newer pointer |
| owned task | pointer has another service ID | any | `configuration_drift`, identity diagnostic | fail with task or installation conflict |
| foreign task | any | any | `configuration_drift`, ownership drift | fail with `MCP_SERVICE_TASK_CONFLICT` |

If task `B` starts before pointer publication, runtime `B` is reported as the
active managed instance because the valid owned task and definition are the
activation authority. Status still reports pointer drift. Retry rebuilds the
pointer without stopping `B`.

Cleanup retains the task-referenced definition, the current-pointer definition,
and any definition referenced by durable runtime state. Other verified
AIHelper-owned definitions may be removed only while the lifecycle lease is
held.

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

```json
{
  "schema_version": 1,
  "operation_id": "lowercase hyphenated UUID",
  "operation": "install",
  "pid": 1234,
  "service_id": null,
  "started_at": "2026-07-22T12:34:56.789Z",
  "state": "active",
  "diagnostic_code": null,
  "finished_at": null
}
```

`operation` is one of `install`, `start`, `stop`, `restart`, `uninstall`,
`upgrade`, or `rollback`. `state` is `active`, `completed`, or `failed`.
`service_id` is nullable because first install may not have allocated it when
metadata is first written. Active records require null `diagnostic_code` and
`finished_at`. Completed records require a non-null `finished_at` and null
diagnostic. Failed records require both a stable diagnostic code and finish
time. No other field is nullable.

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
`runtime.json` atomically. Runtime schema version 1 is exact:

```json
{
  "schema_version": 1,
  "service_id": "lowercase hyphenated UUID",
  "configuration_id": "lowercase hyphenated UUID",
  "phase": "starting",
  "pid": 1234,
  "version": "1.1.0",
  "instance_id": "lowercase hyphenated UUID",
  "endpoint": "http://127.0.0.1:8787/mcp",
  "started_at": "2026-07-22T12:34:56.789Z",
  "updated_at": "2026-07-22T12:34:56.789Z",
  "last_exit": null
}
```

`phase` is `starting`, `ready`, `stopping`, `stopped`, or `failed`. Every field
except `last_exit` is required and non-null. `last_exit` is null for active
phases. It is required for `stopped` and `failed` and has this schema:

```json
{
  "kind": "clean",
  "exit_code": 0,
  "diagnostic_code": null
}
```

`kind` is `clean`, `startup_failure`, or `runtime_failure`. `exit_code` is a
signed 32-bit process exit code. `diagnostic_code` is null only for `clean` and
is a stable non-null code for both failure kinds.

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
    "definition_path": "absolute path",
    "diagnostic_code": null
  },
  "scheduler": {
    "state": "running",
    "last_result": 0,
    "last_result_hex": "0x00000000",
    "last_run_at": null,
    "diagnostic_code": null,
    "hresult": null,
    "hresult_hex": null
  },
  "runtime": {
    "status": "ready",
    "service_id": "uuid",
    "configuration_id": "uuid",
    "version": "1.1.0",
    "instance_id": "uuid",
    "pid": 1234,
    "endpoint": "http://127.0.0.1:8787/mcp",
    "started_at": "timestamp",
    "updated_at": "timestamp",
    "diagnostic_code": null
  },
  "readiness": {
    "status": "ready",
    "http_status": 200,
    "version": "1.1.0",
    "instance_id": "uuid",
    "pid": 1234,
    "diagnostic_code": null
  },
  "lifecycle": {
    "status": "idle",
    "operation": null
  },
  "drift": []
}
```

All top-level fields and all section fields shown above are always serialized.
Nullable fields are encoded as explicit JSON `null`, never omitted.

### Registration section

| Field | Type | Nullability |
| --- | --- | --- |
| `status` | `not_installed \| installed \| configuration_drift \| scheduler_error` | never |
| `service_id` | lowercase hyphenated UUID string | nullable |
| `configuration_id` | lowercase hyphenated UUID string | nullable |
| `task_path` | expected absolute scheduler path string | never |
| `definition_path` | normalized absolute Windows path string | nullable |
| `diagnostic_code` | stable diagnostic string | nullable |

For `not_installed`, valid orphaned `current.json` values are reported;
otherwise both IDs and the definition path are null. Orphaned values do not
change the registration status. `installed` requires non-null IDs and
definition path and a null diagnostic. Drift and scheduler error require a
non-null diagnostic.

### Scheduler section

| Field | Type | Nullability |
| --- | --- | --- |
| `state` | `not_installed \| unknown \| disabled \| queued \| ready \| running \| error` | never |
| `last_result` | signed 32-bit integer | nullable |
| `last_result_hex` | uppercase `0xXXXXXXXX` string | nullable with `last_result` |
| `last_run_at` | UTC RFC 3339 millisecond timestamp | nullable |
| `diagnostic_code` | stable diagnostic string | nullable |
| `hresult` | signed 32-bit integer | nullable |
| `hresult_hex` | uppercase `0xXXXXXXXX` string | nullable with `hresult` |

`not_installed` has null result, run time, diagnostic, and HRESULT fields.
`error` requires a diagnostic and, when COM supplied one, both HRESULT forms.

### Runtime section

| Field | Type | Nullability |
| --- | --- | --- |
| `status` | runtime status enum | never |
| `service_id` | lowercase hyphenated UUID string | nullable |
| `configuration_id` | lowercase hyphenated UUID string | nullable |
| `version` | semantic version string as reported by the binary | nullable |
| `instance_id` | lowercase hyphenated UUID string | nullable |
| `pid` | unsigned 32-bit integer | nullable |
| `endpoint` | absolute loopback URL string | nullable |
| `started_at` | UTC RFC 3339 millisecond timestamp | nullable |
| `updated_at` | UTC RFC 3339 millisecond timestamp | nullable |
| `diagnostic_code` | stable diagnostic string | nullable |

Runtime status is one of `stopped`, `starting`, `running_not_ready`, `ready`,
`stopping`, `restart_backoff`, `failed`, or `identity_mismatch`. PID, version,
and instance ID are considered reliable only when runtime and readiness
identity match; otherwise they remain nullable or describe only the stale
runtime record and the status carries a diagnostic.

### Readiness section

| Field | Type | Nullability |
| --- | --- | --- |
| `status` | `not_checked \| not_ready \| ready \| identity_mismatch \| error` | never |
| `http_status` | unsigned 16-bit HTTP status | nullable |
| `version` | response version string | nullable |
| `instance_id` | lowercase hyphenated UUID string | nullable |
| `pid` | unsigned 32-bit integer | nullable |
| `diagnostic_code` | stable diagnostic string | nullable |

`not_checked` is used only when there is no safe endpoint to probe. A transport
failure is `not_ready`; malformed JSON or an invalid response is `error`; a
well-formed response that does not match durable identity is
`identity_mismatch`.

### Lifecycle section

`status` is `idle` or `busy`. `operation` is null for `idle`. For `busy`, it is
either null when metadata is unreadable, or this exact object:

```json
{
  "operation_id": "uuid",
  "operation": "start",
  "pid": 1234,
  "service_id": "uuid or null",
  "started_at": "2026-07-22T12:34:56.789Z"
}
```

### Drift entries

Each entry has this exact schema:

```json
{
  "field": "action.arguments",
  "kind": "mismatch",
  "expected": "canonical non-secret value or null",
  "actual": "canonical non-secret value or null",
  "diagnostic_code": "MCP_SERVICE_CONFIGURATION_DRIFT"
}
```

`kind` is `missing`, `unexpected`, `mismatch`, or `invalid`. Entries are sorted
by `field`, then `kind`. Expected and actual are nullable strings so all
property types use one deterministic representation. Secret values are null.
Status never emits credentials or environment contents.

### Runtime reduction

The reducer applies this precedence:

1. A responding server with mismatched durable identity is
   `identity_mismatch`.
2. A stop-like active lifecycle operation or runtime `stopping` phase is
   `stopping`.
3. Exact readiness is `ready`.
4. An occupied instance lease or running scheduler task is `starting` when the
   matching runtime phase is starting, otherwise `running_not_ready`.
5. Scheduler `queued` after a nonzero last result, with failed or stale runtime
   and zero restart-setting drift, is `restart_backoff`.
6. A handled failed runtime with no running or queued scheduler task is
   `failed`.
7. All remaining non-running observations are `stopped`.

Corrupt current, definition, runtime, or lifecycle JSON is converted into the
relevant section diagnostic and drift entry. It does not make read-only status
fail or repair the file.

`not_installed`, stopped, drifted, and scheduler-error snapshots are valid
status results with exit code zero. Automation inspects structured state. Only
failure to parse the command or serialize a snapshot is a command error.

## Mutation output

Successful install and start output use typed deterministic structures rather
than generic messages. The exact JSON schema is:

```json
{
  "command": "mcp.service.install",
  "schema_version": 1,
  "changed": true,
  "action": "installed",
  "service_id": "lowercase hyphenated UUID",
  "configuration_id": "lowercase hyphenated UUID",
  "task_path": "absolute scheduler path",
  "endpoint": "http://127.0.0.1:8787/mcp",
  "registration": "installed",
  "runtime": "ready"
}
```

Every field is required and non-null. `command` is `mcp.service.install` or
`mcp.service.start`. Install actions are `installed`, `updated`, or
`unchanged`. Start actions are `started`, `waited`, or `already_ready`.
`registration` is `installed` for every successful mutation. `runtime` uses the
runtime status enum; successful start always reports `ready`, while install
with `--no-start` reports the observed state.

`changed` is true when the command writes a definition or pointer, changes the
task, sends a control shutdown, or calls `RunEx`. Waiting for another command or
observing an already ready instance leaves it false. Text output renders the
same fields in the schema order. `--quiet` suppresses successful output but
never errors.

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

- early classification of every managed command before plugin discovery;
- global-option values that resemble commands and malformed managed syntax;
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
Обнаруживать Task Scheduler configuration drift.
Сохранять полезную scheduler и lifecycle-диагностику.
```

Keep stop, restart, uninstall, bounded restart/backoff, and the combined
lifecycle/Windows VM test item. The task definition and drift logic include the
restart settings in this block, but the roadmap item remains until a real
Scheduler crash test proves nonzero exit, bounded retries, delay, and final
cessation. Keep target-client compatibility unchanged for later manual
verification.

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
- All lifecycle commands run before dynamic plugin discovery; managed startup
  uses explicit immutable configuration before discovery.
- Lifecycle and instance leases recover automatically after process death.
- Exactly one managed HTTP MCP process can run for the user.
- The registered task reads back with bounded restart settings and no
  execution-time limit; operational crash/backoff verification remains open.
- Status separately reports registration, scheduler, runtime, readiness,
  lifecycle operation, and sorted drift.
- Text and JSON contracts are deterministic and contain stable diagnostics.
- Existing manual stdio and HTTP MCP behavior remains compatible.
- Completed roadmap entries are removed only after the full validation suite
  passes.
