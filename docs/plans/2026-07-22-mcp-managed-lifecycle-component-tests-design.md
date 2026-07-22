# Managed MCP Lifecycle Component Tests Design

## Status

The design direction was approved on 2026-07-22. This written specification is
awaiting final user review before implementation planning.

## Goal

Complete the automated lifecycle orchestration coverage for managed MCP without
registering persistent Windows Task Scheduler tasks or changing the production
API.

The new layer is named **in-process lifecycle component integration tests**. It
uses fake `SchedulerAdapter` and `RuntimeControl` boundaries with the real
`LifecycleService`, `ServiceStore`, atomic JSON persistence, temporary
filesystem layout, and file leases.

It is not described as CLI integration, Task Scheduler integration, or
end-to-end testing. Those claims remain reserved for the deferred Windows VM
matrix with a real `ah.exe` and persistent per-user task.

## Scope

This block will:

- reorganize the existing lifecycle tests into an internal test tree;
- add a reusable stateful scenario harness;
- retain the current pure unit and lifecycle component cases;
- add the missing success, failure, retry, recovery, and concurrency scenarios;
- keep all managed state below a temporary directory;
- update the roadmap so automated coverage and deferred Windows VM coverage are
  separate and unambiguous.

This block will not:

- change command behavior, schemas, diagnostics, plugin ABI, or production
  visibility;
- add hidden CLI flags, public test APIs, Cargo test-support features, or new
  runtime configuration;
- register, run, stop, or delete a real Task Scheduler task;
- add a property-based reference model or scenario DSL;
- duplicate parser, text/JSON rendering, managed-process exit, readiness, or COM
  adapter tests that already exist at their appropriate layer;
- perform the deferred Windows VM/manual checks.

## Test architecture

Replace the large inline test module in `lifecycle.rs` with a private descendant
test tree:

```text
src/mcp_service/lifecycle.rs
src/mcp_service/lifecycle/tests/
  mod.rs
  harness.rs
  reducer.rs
  install_start_status.rs
  stop_restart.rs
  uninstall_recovery.rs
  concurrency.rs
```

`lifecycle.rs` declares only `#[cfg(test)] mod tests;`. Because the test tree is
a descendant of the lifecycle module, it can exercise private orchestration
types without widening production visibility.

The pure reducer, error parsing, and property-selection cases live in
`reducer.rs`. Windows-only service orchestration cases live in the scenario
files. `mod.rs` owns only module declarations and narrowly shared imports.

The reorganization is mechanical: existing tests must pass before any new
scenario is added. It may be committed independently so failures in harness
movement are distinguishable from new coverage.

## Lifecycle harness

`LifecycleHarness` owns:

- one `TempDir`;
- `ServicePaths::from_base` rooted below that directory;
- the real `ServiceStore` used by `LifecycleService`;
- a `ScriptedScheduler` implementing `SchedulerAdapter`;
- a `ScriptedRuntimeControl` implementing readiness and identity-aware shutdown;
- deterministic service, configuration, instance, Scheduler-instance, and PID
  identities supplied by scenario builders;
- short test-only start, stop, grace, and polling durations;
- a typed journal of significant adapter calls.

The harness creates valid state through production `ServiceStore` methods.
Tests write raw JSON only when the scenario explicitly requires malformed or
unsupported-version state.

Useful scenario operations include:

```text
install_no_start
install_and_ready
set_owned_task
set_runtime
queue_readiness
hold_instance_lease
release_instance_on_shutdown
release_instance_on_forced_stop
fail_next_scheduler_operation
block_next_run
adapter_events
assert_no_destructive_events
```

Helpers are test-private and describe state or evidence. They must not encode a
second copy of lifecycle decision logic.

## Scripted adapters

`ScriptedScheduler` maintains an observed task, Scheduler instances, typed
one-shot faults, synchronization gates, and an event journal.

Supported fault points are limited to the externally meaningful boundaries:

- inspect;
- register/readback;
- run;
- instance enumeration;
- exact instance stop;
- owned task deletion.

Happy-path adapter calls may perform the minimal state transition that the real
adapter contract guarantees, such as publishing the registered observation,
clearing a stopped instance, or making a deleted task missing. Every transition
can be preceded by a one-shot fault. Registration results may also return an
explicit drifted `ObservedTask` so readback validation is tested independently
from transport failure. Tests do not assert the exact number of polling
inspections; they assert meaningful mutation events and final state.

`ScriptedRuntimeControl` maintains queued readiness observations, queued
shutdown receipts, optional instance-lease release behavior, synchronization
gates, and its own typed event journal.

The harness never attempts to emulate Task Scheduler retry timing, COM races, or
Windows task-state semantics. Those remain adapter/VM responsibilities.

## Assertions and failure contract

Each failure or recovery scenario verifies three dimensions:

1. the stable diagnostic or typed result;
2. the allowed and forbidden adapter mutations;
3. the durable state left for a safe retry.

Tests prefer final invariants over incidental call order. Order is asserted only
across irreversible lifecycle boundaries, for example stop-old before run-new,
or delete-task before semantic metadata cleanup.

Random timestamps and generated UUID values are not snapshotted. Scenarios use
fixed identities where equality or replacement matters and compare relations
such as same service ID, changed configuration ID, or changed instance ID.

Full text and JSON snapshots remain in the command/output test modules. These
component tests inspect typed results and durable documents.

## Automated lifecycle matrix

Existing coverage remains authoritative. The following cases are the required
incremental matrix.

### Install and start

- A default install registers the task, publishes verified metadata, invokes
  one run, and succeeds only after exact readiness.
- Repeating the same ready install does not register or run again.
- A supported configuration update preserves the service ID, creates a new
  configuration ID, stops the exact old instance before activation, and reaches
  exact readiness for the replacement.
- A `--no-start` update does not stop the old live instance or invoke run.
- A register/readback failure does not publish the candidate current pointer;
  a later retry converges.
- Starting an exact ready instance does not invoke run.
- A run failure leaves an installed, retryable registration; a later start can
  converge to ready.
- A foreign or semantically drifted task receives no unsafe run.

### Status

- Representative persisted snapshots cover stopped, ready,
  running-not-ready, failed, identity mismatch, configuration drift, Scheduler
  error, and invalid runtime/lifecycle documents.
- Status preserves sorted drift and the underlying Scheduler/runtime
  diagnostics.
- Status does not change the bytes of current, definition, runtime, or lifecycle
  documents.
- While the lifecycle lease is occupied, status returns immediately with the
  active operation and does not wait for the mutator.

Pure state permutations such as restart-backoff evidence stay in reducer unit
tests instead of being repeated through every persisted-state combination.

### Stop and restart

- A failed control request falls back only to an exact, revalidated Scheduler
  instance.
- A Scheduler stop failure preserves registration and durable state; retry after
  removing the fault succeeds.
- Restart of an already stopped service performs the start phase and returns
  `started`.
- Stop failure during restart prevents run.
- Start failure after a successful restart stop leaves an installed, honest
  stopped/failed state and supports recovery through a later start.
- A post-stop task drift prevents run.
- Existing exact identity, unsafe PID/drift, timeout, and reused-instance UUID
  cases remain green.

### Uninstall and recovery

- A running owned service is stopped, the owned task is deleted, and verified
  semantic metadata is removed.
- Executable, configuration, logs, plugins, unexpected sentinel files, and lock
  anchors are preserved.
- An owned-task deletion failure starts no metadata cleanup and leaves enough
  evidence for a successful retry.
- A missing task with stale or partially removed metadata converges on retry.
- An occupied orphan without exact identity blocks cleanup.
- A foreign task receives no stop or delete, and foreign evidence is never used
  as ownership authority.
- Existing malformed, newer-schema, and already-uninstalled cases remain green.

### Concurrency

One representative deterministic scenario proves the shared lifecycle lock:

1. Install a stopped managed service.
2. Begin restart and block its run call after the stop phase while it still owns
   the lifecycle lease.
3. Observe status from another thread and require `busy` with
   `operation=restart` without waiting for restart completion.
4. Invoke a second mutating command and require `MCP_SERVICE_BUSY` with no
   additional mutation event.
5. Release the run gate and allow restart to reach exact readiness.

Synchronization uses channels or barriers. Sleep is not used to establish
ordering; bounded timeouts only prevent a broken test from hanging forever.

## Validation

Validation runs in layers:

1. Existing lifecycle tests immediately after mechanical relocation.
2. Focused reducer and scenario modules while adding coverage.
3. Existing managed process integration tests.
4. Non-persistent Task Scheduler COM smoke.
5. Required workspace checks:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

If the `ah` MCP wrapper reaches its transport deadline, the exact command may be
rerun directly with a larger process timeout, and the fallback must be reported.

## Documentation and roadmap

After the automated matrix passes, remove the combined roadmap item for unit,
integration, and Windows VM tests. Do not retain a completed checkbox.

Replace it with one explicit open item:

```text
- [ ] Выполнить Windows VM/manual end-to-end lifecycle matrix через реальный
      ah.exe и persistent per-user Task Scheduler task.
```

The open VM matrix continues to cover standard-user install without elevation
or a stored password, real CLI install/reinstall/`--no-start`/start/status/stop/
restart/uninstall, logon and missed triggers, single-instance enforcement,
control-to-Scheduler fallback, external drift, cross-process CLI concurrency,
crash retry/backoff, and preservation of user files after uninstall.

The design document and basic-memory milestone state that the completed layer is
in-process component integration with fake external adapters and real temporary
durable primitives. They must not imply that real Task Scheduler lifecycle
behavior has passed.
