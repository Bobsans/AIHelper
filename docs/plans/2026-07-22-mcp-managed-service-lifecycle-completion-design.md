# Managed MCP Service Lifecycle Completion Design

## Status

Approved for implementation on 2026-07-22.

This design completes the remaining first-version managed MCP lifecycle
commands on top of the foundation committed in `af19bb3`:

- `ah mcp service stop`;
- `ah mcp service restart`;
- `ah mcp service uninstall`.

The existing foundation remains authoritative for task naming, immutable
definitions, durable state schemas, path identity, readiness, lifecycle and
instance leases, early routing, deterministic output, and Task Scheduler
property drift.

## Scope

This block adds:

- identity-aware graceful shutdown;
- bounded Task Scheduler fallback for a proven managed instance;
- restart under one lifecycle lease;
- ownership-gated task deletion;
- idempotent managed metadata cleanup;
- deterministic text and JSON results;
- unit, integration, and non-persistent Windows COM coverage;
- command reference, AI manual, recipe, and roadmap updates.

This block does not add:

- direct PID termination;
- a new helper executable or transaction journal;
- deletion of user configuration, logs, binaries, plugins, or arbitrary files;
- real persistent Task Scheduler mutation in normal workspace tests;
- the deferred Windows VM, crash/backoff, or target-client compatibility
  matrix.

## Chosen approach

Use an identity-gated lifecycle state machine inside the existing
`LifecycleService`.

Public commands acquire the lifecycle lease exactly once through
`with_operation`. Internal methods do not reacquire it:

```text
stop -> with_operation(Stop) -> stop_locked
restart -> with_operation(Restart) -> restart_locked
                                    -> stop_locked
                                    -> start_locked
uninstall -> with_operation(Uninstall, RemoveLifecycleOnSuccess)
                                      -> uninstall_locked
                                      -> stop_locked
```

This avoids a lock gap between restart phases and avoids composing public
commands that would attempt nested lock acquisition.

The existing registration and durable artifacts continue to provide enough
recovery evidence. A new durable transaction journal would duplicate those
signals and is deferred until the updater requires a cross-process handoff.

## Authority and identity proofs

Destructive operations use three distinct proof levels.

### Owned registration

An `OwnedRegistration` requires:

- the expected per-user task path;
- valid AIHelper source and URI markers;
- a valid `TaskMarker`;
- marker `definition_path` equals the canonical managed path derived from its
  configuration ID;
- the current Windows user SID;
- exact service and configuration UUID formatting.

This proof permits registration inspection and conditional deletion. A foreign
or invalid marker never permits stop or delete.

### Safe task execution

A `SafeTaskExecution` additionally requires:

- the marker references a valid immutable definition inside the managed store;
- definition service and configuration IDs match the marker;
- task action executable, arguments, working directory, principal, trigger,
  single-instance policy, and relevant settings match the canonical task spec;
- the definition belongs to the current user and uses the canonical runtime and
  instance-lock paths.

This proof permits forced Scheduler fallback. An owned but safety-drifted task
may be inspected and, when proven inactive, deleted by uninstall, but it is not
force-stopped.

### Exact live instance

An `ExactLiveInstance` additionally requires:

- durable runtime service and configuration IDs match the selected definition;
- readiness matches runtime PID and instance ID;
- readiness version and endpoint match the definition;
- the expected instance lease is occupied.

Only this proof permits `POST /control/shutdown`.

## Scheduler adapter extensions

Extend `SchedulerAdapter` with typed operations that carry expected identity,
not a bare path:

```text
instances(expected_owned_task) -> [SchedulerInstance]
stop_instance(expected_owned_task, stop_target) -> SchedulerStopReceipt
delete_owned(expected_registration) -> SchedulerDeleteReceipt
```

`SchedulerInstance` contains the Scheduler instance ID, engine PID when
available, and state. The Windows adapter obtains instances from the registered
task. A running instance may be stopped only when its engine PID equals the
exact durable PID. A queued instance without an engine PID may be cancelled by
its Scheduler instance ID only when the task is canonical and owned, no running
instance exists, and the managed instance lease is free.

The typed stop target is:

```text
Running {
  scheduler_instance_id,
  expected_pid
}
Queued {
  scheduler_instance_id
}
```

The adapter re-enumerates the expected task instances and requires the same
Scheduler instance ID immediately before `IRunningTask::Stop`. A running target
must also retain the expected engine PID. A queued target must remain queued,
must not acquire an engine PID, and is rejected if any running sibling appears.

Before either stop or delete, the Windows adapter reopens the task in the same
COM session and revalidates source, URI, and `TaskMarker`. Before stop it also
revalidates the canonical execution properties. A task that changed after the
core inspection returns `MCP_SERVICE_TASK_CHANGED` without mutation.

Task Scheduler does not provide an atomic compare-and-delete primitive. A
malicious same-user process racing the final name-based deletion is outside the
threat model. Ordinary concurrent lifecycle commands remain serialized by the
per-user lifecycle lease, and non-adversarial external edits are detected by
the adapter revalidation.

The adapter never calls `TerminateProcess` and never searches globally for an
`ah.exe` process.

## Runtime control boundary

Move control shutdown behind a testable runtime-control abstraction shared
with readiness:

```text
inspect(definition, runtime) -> ReadinessSection
shutdown(definition, exact_instance_id) -> ShutdownReceipt
```

The production implementation keeps the existing loopback-only HTTP behavior.
Shutdown requires HTTP `202 Accepted` from the expected endpoint. Transport,
HTTP, or identity failure does not directly fail stop when a safe Scheduler
fallback remains available.

No shutdown request is sent when readiness reports another identity.

## Stop behavior

`ah mcp service stop` requires an installed owned registration. A missing task
returns `MCP_SERVICE_NOT_INSTALLED`; an installed but already inactive service
is an idempotent success.

The internal engine accepts an explicit policy:

```text
RequireRegistration
AllowExactOrphan
```

Public stop and restart use `RequireRegistration`. Uninstall uses
`AllowExactOrphan`: when the task is missing, a valid current definition plus
exact runtime/readiness identity may receive control shutdown, but Scheduler
fallback is unavailable. An orphan with occupied instance lease and no exact
identity returns `MCP_SERVICE_STOP_UNSAFE`.

The command uses one total 15-second deadline:

1. Acquire the lifecycle lease and write active `operation=stop`.
2. Resolve the owned task, marker definition, current pointer, runtime, task
   instances, readiness, and instance lease.
3. Reject a foreign task. Preserve current/task identity conflicts as explicit
   state or installation errors.
4. If the task has no running or queued instance, the instance lease is free,
   and the old exact readiness identity is absent, return `already_stopped`.
5. If an exact live instance exists, send identity-aware control shutdown.
6. Wait up to five seconds for graceful disappearance.
7. If the old process remains, require `SafeTaskExecution`, revalidate it in the
   adapter, match the durable PID to one running Scheduler instance, and stop
   that exact instance. A process-free queued instance may instead be cancelled
   under the queued-instance rule above.
8. Use the remaining deadline to prove quiescence.

Successful quiescence requires all of the following:

- the instance lease can be acquired and retained as a proof guard;
- no Scheduler instance is running or queued;
- the previous exact instance UUID no longer answers readiness.

The proof guard remains held until the enclosing operation completes. This
prevents a delayed Scheduler start from acquiring the managed instance lease
during final verification.

The successful actions are:

- `already_stopped` with `changed=false`;
- `stopped` after controlled shutdown with `changed=true`;
- `forced_stopped` after Scheduler fallback with `changed=true`.

A foreign HTTP server on the configured port is never stopped. If the owned
task is inactive and the instance lease is free, the managed service is stopped
even though a later status snapshot may still report endpoint identity
mismatch.

Forced stop does not rewrite stale runtime as a clean exit. The existing status
reducer derives `stopped` from Scheduler, lease, and readiness evidence while
preserving the last runtime record for diagnostics.

## Restart behavior

`ah mcp service restart` holds one lifecycle lease across both phases.

Before stopping anything it requires the registration, current pointer,
definition, and canonical task configuration to agree. This prevents a known
drift from turning into an avoidable outage when the subsequent start would be
rejected.

The command:

1. Records the old exact instance ID when available.
2. Calls `stop_locked` and retains its proof guard.
3. Revalidates task ownership and canonical properties.
4. Releases the proof guard immediately before `RunEx`.
5. Calls the existing internal start path and waits for exact readiness.
6. Requires a new instance ID when an old ID was known.

If the service was already stopped, restart performs only the start phase. Its
action is `started`. A running service that completes both phases returns
`restarted`.

Stop failure prevents `RunEx`. If stop succeeds and start fails, the service
remains installed and stopped, starting, or failed according to observed state.
The command does not attempt to recreate the previous process and does not
report rollback.

A CLI crash between stop and start leaves a valid installed stopped service; a
later start or restart converges safely. A retry after `RunEx` still requires a
new exact ready identity and never accepts the old UUID as a successful restart.

## Uninstall behavior

`ah mcp service uninstall` is idempotent and removes only the managed service
registration and semantic metadata.

The command:

1. Acquires the lifecycle lease and writes active `operation=uninstall`.
2. Rejects a foreign task without stopping anything or deleting metadata.
3. Resolves the owned task as activation authority. If the task is missing,
   valid current metadata may identify an orphan managed instance.
4. Stops the managed instance through
   `stop_locked(AllowExactOrphan)` and retains the instance proof guard. The
   missing-task branch permits exact control shutdown only and never Scheduler
   fallback.
5. Revalidates task ownership immediately before conditional deletion.
6. Deletes the owned registration and confirms that a subsequent inspection is
   `Missing`.
7. Builds the verified definition deletion set described below.
8. Idempotently removes semantic metadata in this order:
   - `runtime.json`;
   - the verified definition deletion set;
   - `current.json`;
   - `lifecycle.json` last.
9. Releases the proof guard and lifecycle lease.

The instance proof guard is the activation fence between stop and task deletion;
the task does not need to be disabled as a separate persistent mutation.

When the task is already missing:

- free instance lease and no metadata returns `already_uninstalled` with
  `changed=false`;
- stale semantic metadata is removed and returns `uninstalled` with
  `changed=true`;
- an exact live orphan may receive controlled shutdown;
- an occupied instance lease without exact identity returns
  `MCP_SERVICE_STOP_UNSAFE` and preserves metadata.

Malformed `current.json`, `runtime.json`, and `lifecycle.json` at their fixed
semantic paths may be removed only after the task is missing and the instance
lease is free. A newer unsupported schema is preserved and returns
`MCP_SERVICE_STATE_INVALID`.

The definition deletion set contains only:

- the canonical definition path referenced by a valid owned task marker;
- the canonical definition path referenced by a valid current pointer;
- the canonical definition path derived from a valid runtime identity when its
  service ID matches the trusted task or current service ID;
- other fully valid definitions whose user SID, service ID, canonical runtime
  path, canonical instance-lock path, and canonical UUID filename match the
  same trusted per-user service.

Every deletion candidate must resolve exactly to
`<managed base>/definitions/<matching configuration UUID>.json` after Windows
path normalization. A marker, pointer, runtime identity, or file whose derived
path is outside that directory is drift or unsafe state and never authorizes
deletion.

A referenced definition path remains eligible even when that exact definition
file is malformed, because the valid marker or pointer supplies its identity.
An unreferenced malformed UUID-named file is not ownership proof and is
preserved as unexpected. If no trusted service ID survives, only explicitly
referenced paths are removed. The command never recursively deletes arbitrary
directory contents.

Successful uninstall removes, when ownership can be proven:

- `current.json`;
- `runtime.json`;
- `lifecycle.json`;
- the verified AIHelper definition deletion set;
- the owned Task Scheduler registration.

It preserves:

- `lifecycle.lock` and `instance.lock` as permanent empty lease anchors;
- the managed base directory when those anchors remain;
- configuration and plugin settings;
- logs;
- `ah.exe` and plugin DLLs;
- every path outside the managed service store;
- unexpected files in the managed store.

The generic operation wrapper gains a success policy. Normal commands write a
completed lifecycle record. Successful uninstall removes `lifecycle.json`
instead of recreating it after cleanup. Failed uninstall writes a failed record
when the metadata store remains writable.

## Output contracts

This document extends the authoritative mutation schema version 1 with new
discriminator variants; it does not change any existing install or start
combination. The complete `command` and `action` union is:

| `command` | Allowed `action` values | Required successful `runtime` |
| --- | --- | --- |
| `mcp.service.install` | `installed`, `updated`, `unchanged` | observed runtime status |
| `mcp.service.start` | `started`, `waited`, `already_ready` | `ready` |
| `mcp.service.stop` | `already_stopped`, `stopped`, `forced_stopped` | `stopped` |
| `mcp.service.restart` | `started`, `restarted` | `ready` |

All fields remain required and non-null for this mutation schema. Stop and
restart use its deterministic field order:

```json
{
  "command": "mcp.service.stop",
  "schema_version": 1,
  "changed": true,
  "action": "stopped",
  "service_id": "lowercase hyphenated UUID",
  "configuration_id": "lowercase hyphenated UUID",
  "task_path": "absolute scheduler path",
  "endpoint": "http://127.0.0.1:8787/mcp",
  "registration": "installed",
  "runtime": "stopped"
}
```

Existing install/start JSON remains byte-shape compatible apart from values that
already varied by operation result.

Uninstall uses a separate removal output so the existing install and start
contract remains unchanged:

```json
{
  "command": "mcp.service.uninstall",
  "schema_version": 1,
  "changed": false,
  "action": "already_uninstalled",
  "service_id": null,
  "configuration_id": null,
  "task_path": "absolute scheduler path",
  "endpoint": null,
  "registration": "not_installed",
  "runtime": "stopped"
}
```

Every uninstall field is present. Identity fields are nullable only when no
valid last-known identity survives. Actions are `uninstalled` and
`already_uninstalled`.

Text output renders the same fields in schema order. `--quiet` suppresses only
successful output.

## Stable diagnostics

Reuse existing diagnostics for unsupported platform, not installed, lifecycle
busy, task conflict, installation conflict, configuration drift, invalid state,
identity mismatch, and Scheduler HRESULT failures.

Add:

```text
MCP_SERVICE_STOP_UNSAFE
MCP_SERVICE_STOP_TIMEOUT
MCP_SERVICE_TASK_CHANGED
MCP_SERVICE_RESTART_FAILED
MCP_SERVICE_UNINSTALL_INCOMPLETE
```

`MCP_SERVICE_STOP_UNSAFE` means no exact identity or canonical owned Scheduler
instance can be proven, so no unknown process was terminated.

`MCP_SERVICE_STOP_TIMEOUT` means shutdown or Scheduler stop was attempted but
the combined quiescence proof did not complete before the total deadline.

`MCP_SERVICE_TASK_CHANGED` means ownership or safe execution properties changed
between inspection and the destructive adapter call.

`MCP_SERVICE_RESTART_FAILED` is used when the post-start instance is not new or
the compound restart invariant fails. Underlying start and Scheduler failures
retain their more specific existing codes.

`MCP_SERVICE_UNINSTALL_INCOMPLETE` means task deletion succeeded or metadata
cleanup began but the requested final state could not be fully established.

All COM failures continue to include operation, signed HRESULT, and uppercase
hex HRESULT without depending on localized Windows messages.

## Crash reconciliation

| Last durable/external state | Retry behavior |
| --- | --- |
| Control shutdown accepted | Observe stopping, stopped, or a free lease and continue |
| Scheduler instance stopped | Re-prove quiescence; stop is idempotent |
| Restart stopped before `RunEx` | Start the installed task |
| Restart called `RunEx` | Require exact readiness and a new identity |
| Uninstall stopped before delete | Revalidate ownership and delete |
| Task deleted before cleanup | Missing task authorizes idempotent semantic cleanup when lease is free |
| Partial metadata cleanup | Missing known files are success; continue in fixed order |
| Cleanup failed after task deletion | Do not recreate the task; report incomplete and allow retry |

No recovery branch terminates a process based only on PID, executable name, a
stale runtime file, or an answering loopback port.

## Tests

### Pure and fake-adapter tests

- route stop, restart, and uninstall before plugin discovery;
- deterministic help, text, JSON, quiet, and AI manual output;
- already-stopped stop;
- exact graceful shutdown;
- accepted shutdown with delayed lease release;
- control transport or HTTP failure followed by exact Scheduler fallback;
- readiness mismatch receives no shutdown request;
- foreign task receives no stop or delete;
- safety drift blocks forced stop;
- durable PID must match a Scheduler instance;
- process-free queued backoff can be cancelled without targeting a PID;
- stop success still times out when the instance lease remains occupied;
- restart holds one lifecycle operation and obtains a new instance UUID;
- stop failure prevents `RunEx`;
- start failure after stop preserves installed registration;
- pre- and post-stop drift blocks restart safely;
- uninstall running, stopped, missing, and partially cleaned states;
- deletion failure preserves semantic metadata;
- task missing plus stale metadata converges to clean state;
- occupied orphan lease without identity blocks cleanup;
- second uninstall is an unchanged success;
- configuration, logs, binary, plugins, lock anchors, and unexpected files are
  preserved.

### Process tests

- a managed runner releases its instance lease after controlled shutdown;
- the uninstall proof guard prevents a duplicate managed runner from becoming
  active between stop and task deletion;
- restart observes a different instance UUID.

### Windows adapter tests

Normal workspace tests construct typed COM definitions and exercise pure
ownership/instance mapping without registering a persistent task. Opt-in or VM
tests may create a uniquely named task with a cleanup guard to verify real
instance stop and conditional deletion.

The existing real crash/restart/backoff, combined lifecycle Windows VM, and
target-client compatibility roadmap items remain open until their separate
manual or opt-in validation succeeds.

## Documentation and roadmap

Update:

- `docs/reference/mcp.md`;
- `docs/agents/recipes/mcp-http.md`;
- `ah ai info` host command entries;
- early command help and integration tests.

After implementation and automated validation, remove only these Stage 2
roadmap entries:

```text
Реализовать `ah mcp service stop`.
Реализовать `ah mcp service restart`.
Реализовать `ah mcp service uninstall`.
```

Keep restart/backoff and combined lifecycle/Windows VM testing open.

## Acceptance criteria

- Stop never sends control shutdown without exact runtime/readiness identity.
- Forced fallback never targets a foreign, drifted, or PID-unmatched Scheduler
  instance.
- Successful stop proves lease, Scheduler, and old-readiness quiescence.
- Restart holds one lifecycle lease and produces a new exact ready identity.
- Uninstall conditionally deletes only an owned task and proves it missing.
- Successful uninstall removes semantic service metadata while preserving all
  explicitly excluded data and lock anchors.
- Repeated stop and uninstall operations are deterministic and idempotent.
- Partial uninstall and restart states converge safely on retry.
- User-facing behavior is documented and exposed through `ah ai info`.
- Relevant tests, workspace formatting, workspace tests, and locked build pass.
