# Managed MCP Restart Policy Design

## Status

Approved for implementation on 2026-07-22.

This design completes the first-version managed MCP restart-on-failure policy
on top of the lifecycle implementation committed in `3ee0a77`.

## Goal

Configure a bounded Windows Task Scheduler restart policy, keep Task Scheduler
as the sole retry controller, and expose `restart_backoff` without claiming
more certainty than the Windows API provides.

The fixed first-version contract is:

- three automatic restart attempts after the initial task launch;
- an ISO 8601 restart interval of `PT1M`;
- fatal managed exits are nonzero so Scheduler can treat them as failures;
- clean managed shutdown remains a zero exit and must not request a retry.

## Scope

This block adds or strengthens:

- named canonical restart-policy constants;
- Task Scheduler definition write/readback and semantic-drift coverage for both
  restart count and restart interval;
- conservative `restart_backoff` inference from Scheduler and durable managed
  runtime evidence;
- deterministic unit, process, and non-persistent Windows COM tests;
- command reference, AI recipe, and roadmap documentation.

This block does not add:

- an in-process retry supervisor;
- a durable retry counter, attempt number, countdown, or next-retry timestamp;
- dependency on the Task Scheduler Operational event log;
- persistent Task Scheduler registration in automated workspace tests;
- the deferred Windows VM crash/restart matrix.

## Chosen approach

Windows Task Scheduler remains the only component that schedules retries.
AIHelper owns the canonical task definition, exits nonzero after fatal managed
failures, and derives a conservative presentation status from available
evidence.

The canonical task definition uses named constants equivalent to:

```text
MANAGED_RESTART_COUNT = 3
MANAGED_RESTART_INTERVAL = "PT1M"
```

The same constants are used by desired-task construction and tests. The Windows
adapter continues to write and read the typed `RestartCount` and
`RestartInterval` properties. Semantic comparison treats either mismatch as
configuration drift.

No second retry loop is introduced. A custom supervisor would duplicate
Scheduler ownership, risk double restarts, and require new cross-process retry
state. Event-log observation is also unsuitable as a required source because
the log can be disabled or unavailable.

## Windows contract boundary

Microsoft documents `RestartCount` as the number of restart attempts and
requires `RestartCount` and `RestartInterval` together. `PT1M` is the minimum
valid interval. The fixed values are therefore a valid bounded policy.

The public Task Scheduler state does not expose a restart reason. In particular,
`TASK_STATE_QUEUED` states only that queued instances exist; it does not prove
that an instance is waiting for the restart interval. `LastTaskResult` exposes
the last result but does not provide an attempt number or next-retry time.

Accordingly, AIHelper documents `restart_backoff` as an inference, not a native
Windows phase or countdown.

References:

- <https://learn.microsoft.com/en-us/windows/win32/taskschd/tasksettings-restartcount>
- <https://learn.microsoft.com/en-us/windows/win32/taskschd/tasksettings-restartinterval>
- <https://learn.microsoft.com/en-us/windows/win32/taskschd/taskschedulerschema-restartonfailure-settingstype-element>
- <https://learn.microsoft.com/en-us/windows/win32/api/taskschd/ne-taskschd-task_state>
- <https://learn.microsoft.com/en-us/windows/win32/api/taskschd/nf-taskschd-iregisteredtask-get_lasttaskresult>

## Status reduction

Readiness and live-instance evidence retain their existing precedence. A queued
task is reported as `restart_backoff` only when all available observations are
consistent with a Scheduler-owned retry:

```text
scheduler state is queued
and the observed restart policy is canonical and not drifted
and the durable managed runtime records a fatal failure
and the last task result is known and nonzero
and no exact ready instance exists
and the managed instance lease is free
```

A queued task without this evidence is reported through the existing starting
or not-ready path. In particular:

- queued after a clean stopped runtime is not `restart_backoff`;
- a zero or unknown task result is not sufficient;
- a live instance or exact readiness takes precedence;
- drifted restart settings do not support a policy-backed inference.

The reducer preserves the underlying Scheduler result and managed runtime
diagnostic. No new public JSON field or diagnostic code is required.

## Exit behavior

The managed runner keeps the existing process contract:

- fatal startup or runtime failure records durable `failed` state and returns a
  nonzero process exit;
- clean control shutdown records `stopped` state and returns zero;
- a duplicate runner that safely declines to start does not create an
  in-process restart loop.

Task Scheduler decides whether the configured restart policy reacts to the
process result. The exact Windows reaction is covered by the deferred VM test,
not asserted from the in-memory task definition alone.

## Error handling

No new stable error code is added.

- Restart count or interval mismatch remains task configuration drift.
- COM property read/write failures remain Scheduler adapter failures with the
  existing operation and HRESULT diagnostics.
- Fatal managed failures retain their existing runtime diagnostic and nonzero
  exit behavior.
- Insufficient evidence for `restart_backoff` changes only the derived status;
  it is not itself an error.

## Automated validation

Unit coverage must prove:

- the canonical spec contains exactly `3` and `PT1M`;
- restart-count and restart-interval drift are detected independently and in
  deterministic order;
- queued fatal failure with a nonzero task result and canonical policy reduces
  to `restart_backoff` when no live evidence exists;
- clean stopped, zero/unknown result, live instance, readiness, or restart
  policy drift prevents the backoff inference;
- transition to running or ready leaves backoff.

Process integration coverage must preserve the existing proof that fatal
managed exits are nonzero and clean control shutdown exits zero. Add focused
coverage only where the current tests do not already prove the durable failure
record and exit result together.

The Windows COM smoke test remains non-persistent:

1. Connect to Task Scheduler.
2. Create an in-memory `ITaskDefinition` with `NewTask`.
3. Populate the canonical definition.
4. Read back both `RestartCount == 3` and
   `RestartInterval == "PT1M"` through the typed adapter path.
5. Never call `RegisterTaskDefinition`.

After focused checks pass, run the applicable workspace format, test, and build
checks.

## Documentation and roadmap

The command reference and AI recipe describe:

- three automatic restart attempts with a one-minute interval;
- Scheduler ownership of retry timing and limits;
- nonzero fatal exits and zero clean exits;
- `restart_backoff` as a conservative inference;
- the absence of an attempt counter or retry countdown.

After automated checks pass, remove the completed roadmap entry for configuring
bounded restart-on-failure and backoff. Keep the Windows VM lifecycle item open
and make its deferred acceptance matrix explicit:

- one initial fatal launch followed by no more than three restart launches;
- intervals approximately matching `PT1M`;
- no further launch after the third retry;
- no restart after a clean zero exit;
- observation of actual task states during the delay without requiring
  `TASK_STATE_QUEUED` as a platform contract.
