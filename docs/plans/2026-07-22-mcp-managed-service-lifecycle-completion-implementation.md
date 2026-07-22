# MCP Managed Service Lifecycle Completion Implementation Plan

## Goal

Complete the first-version managed HTTP MCP lifecycle on Windows by adding safe,
idempotent `stop`, `restart`, and `uninstall` commands to the foundation in
`af19bb3`.

The normative behavior, safety proofs, output contracts, and recovery rules are
defined in
`2026-07-22-mcp-managed-service-lifecycle-completion-design.md`. This plan
sequences the implementation and tests without changing that design.

## Completion boundary

This block is complete when:

- `ah mcp service stop`, `restart`, and `uninstall` route before plugin discovery;
- graceful shutdown is sent only to an exact managed instance;
- forced fallback targets only a revalidated canonical owned Scheduler instance;
- restart holds one lifecycle lease and returns only after a new exact instance
  is ready;
- uninstall conditionally removes only the owned task and verified semantic
  metadata;
- repeated and partially completed operations converge safely;
- existing install, start, status, manual serve, JSON fields, and plugin ABI
  behavior remain compatible;
- user-facing reference, AI recipe, AI manual, and roadmap are updated;
- focused and workspace checks pass;
- only the three implemented Stage 2 lifecycle checklist entries are removed.

The real Task Scheduler crash/backoff test, combined Windows VM lifecycle
matrix, and target-client compatibility checks remain open.

## Task 1: Extend command and output contracts

**Files**

- Modify: `src/mcp_service/command.rs`
- Modify: `src/mcp_service/model.rs`
- Modify: `src/mcp_service/output.rs`
- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/cli.rs`

**Implementation**

1. Add `Stop`, `Restart`, and `Uninstall` variants to the early service command
   parser. They accept only existing global output options.
2. Add all three subcommands to early help so help and parse failures continue
   to bypass configuration and plugin discovery.
3. Extend mutation schema version 1 with only the approved stop and restart
   command/action discriminator combinations. Keep every existing install/start
   serialized field and value rule unchanged.
4. Add a separate `UninstallOutput` with every field present and nullable
   last-known service ID, configuration ID, and endpoint.
5. Add deterministic text and JSON emitters. Text uses schema field order;
   `--quiet` suppresses successful output only.
6. Route the new command variants through the Windows lifecycle service and the
   existing stable unsupported-platform diagnostic on other platforms.

**Tests**

- Parse every new command with text, JSON, quiet, cwd, and limit global options.
- Reject unsupported flags and missing/unknown service subcommands.
- Assert command help lists all six public lifecycle commands.
- Snapshot stop, restart, uninstall, and already-uninstalled JSON shapes.
- Assert existing install and start JSON values remain unchanged.

## Task 2: Make runtime control testable

**Files**

- Modify: `src/mcp_service/readiness.rs`
- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/mod.rs` only if exports change

**Implementation**

1. Extend the readiness boundary into a `RuntimeControl` abstraction covering
   both `inspect` and identity-aware `shutdown`.
2. Preserve the current bounded loopback readiness implementation and Host,
   endpoint, version, PID, and instance-ID checks.
3. Move the existing free control-shutdown HTTP function behind the production
   implementation. Require `202 Accepted` for a submitted shutdown.
4. Return a typed shutdown receipt so orchestration can distinguish submitted
   control from transport/HTTP failure without parsing prose.
5. Convert `LifecycleService` to depend on the combined control abstraction.
   Preserve install configuration-replacement behavior while making it use the
   same testable control port.
6. Add a programmable fake that records shutdown targets and supports readiness
   and shutdown result sequences without sleeping or network I/O.

**Tests**

- Exact identity submits the expected instance UUID once.
- Readiness identity mismatch never submits shutdown.
- Transport and non-202 responses become stable control failures available to
  Scheduler fallback.
- Existing install replacement and start readiness tests remain green.

## Task 3: Add identity-gated Scheduler instance operations

**Files**

- Modify: `src/mcp_service/scheduler.rs`
- Modify: `src/mcp_service/windows_scheduler.rs`
- Modify: `src/mcp_service/lifecycle.rs`

**Implementation**

1. Add typed scheduler models:
   - expected owned registration identity;
   - `SchedulerInstance` with normalized instance UUID, state, and optional
     engine PID;
   - `SchedulerStopTarget::Running { instance_id, expected_pid }`;
   - `SchedulerStopTarget::Queued { instance_id }`;
   - stop and delete receipts.
2. Extend `SchedulerAdapter` with instance enumeration, conditional instance
   stop, and conditional owned-task deletion. Never expose destructive methods
   that accept only a task path.
3. Keep inspect/register/run signatures compatible where possible. Share marker
   parsing and ownership validation instead of duplicating ad-hoc checks.
4. In the Windows adapter, enumerate `IRunningTask` values from the exact
   registered task and normalize Scheduler instance GUIDs deterministically.
5. Before stopping, reopen the registered task in the same COM session,
   revalidate source, URI, marker, and canonical safe execution properties, then
   re-enumerate instances:
   - running target requires the same instance UUID and engine PID;
   - queued target requires the same queued instance UUID, no engine PID, no
     running sibling, and the core-supplied free-lease precondition.
6. Stop only the selected `IRunningTask`; do not call `TerminateProcess` and do
   not search processes by executable name.
7. Before deletion, reopen the task in the same COM session and revalidate the
   expected registration marker immediately before `DeleteTask`.
8. Map task disappearance idempotently. Preserve operation, signed HRESULT, and
   uppercase hex HRESULT on all other COM errors.
9. Extend the fake adapter with observable stop/delete calls and programmable
   task/instance transitions.

**Tests**

- Deterministic instance GUID and state mapping.
- Running PID match succeeds; UUID or PID mismatch returns task-changed/unsafe
  without stop.
- Queued instance cancellation succeeds only with its exact ID and no running
  sibling.
- Foreign, malformed, and safety-drifted tasks receive no stop.
- Marker replacement between inspect and stop/delete is rejected.
- Delete missing is idempotent; foreign task is preserved.
- Windows-only non-persistent COM smoke test enumerates relevant typed
  interfaces and validates pure mapping helpers.

## Task 4: Implement the shared stop engine

**Files**

- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/lock.rs` only if a proof-guard helper is useful
- Modify: `src/mcp_service/model.rs`
- Modify: `src/mcp_service/output.rs`

**Implementation**

1. Add typed internal proof structures for owned registration, safe task
   execution, exact live instance, and a retained instance-lease guard.
2. Centralize authority resolution across task marker, current pointer,
   immutable definition, runtime, readiness, and current user SID. Reuse the
   foundation rule that the owned registered task is activation authority.
3. Add `StopPolicy::RequireRegistration` and
   `StopPolicy::AllowExactOrphan`. Public stop and restart use the first;
   uninstall uses the second.
4. Add configurable total, graceful, and poll durations to `LifecycleService`
   so production uses the approved 15/5-second limits while unit tests complete
   immediately.
5. Implement `stop_locked` without acquiring the lifecycle lease:
   - return already-stopped only when Scheduler, instance lease, and old exact
     readiness prove inactivity;
   - submit control shutdown only for exact live identity;
   - wait for graceful release;
   - on failure, require canonical safe task execution and select the exact
     running PID or safe process-free queued target;
   - call the conditional Scheduler stop;
   - use the remaining total deadline to prove quiescence.
6. Quiescence must retain the acquired instance lease and require no running or
   queued Scheduler instance plus disappearance of the previous exact UUID.
7. Leave stale runtime untouched after forced stop. Do not fabricate a clean
   exit.
8. Add the public stop wrapper through one `with_operation(Stop)` and emit the
   approved action and runtime values.
9. Return `MCP_SERVICE_STOP_UNSAFE` before mutation when identity proof is
   insufficient, and `MCP_SERVICE_STOP_TIMEOUT` when an attempted stop cannot be
   proven complete.

**Tests**

- Already stopped returns unchanged success.
- Exact controlled stop waits for lease release and returns `stopped`.
- Control failure falls back to one exact running Scheduler instance.
- Process-free queued backoff is cancelled without targeting a PID.
- Foreign endpoint receives no POST and remains untouched.
- Foreign task, safety drift, stale runtime, PID mismatch, and task replacement
  produce zero unsafe stop calls.
- Scheduler stop success with occupied instance lease still times out.
- One total deadline is shared across graceful and forced phases.
- Public stop requires an installed registration.
- Concurrent lifecycle command observes the existing stable busy diagnostic.

## Task 5: Implement restart under one lease

**Files**

- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/model.rs`
- Modify: `src/mcp_service/output.rs`

**Implementation**

1. Extend the internal start result with the exact ready instance UUID while
   preserving public start output.
2. Add `restart_locked` and a public `with_operation(Restart)` wrapper. Do not
   invoke public stop or public start.
3. Before stop, require agreement among current pointer, owned task marker,
   immutable definition, and canonical task properties.
4. Record the old exact instance UUID when available, call
   `stop_locked(RequireRegistration)`, and retain its instance proof guard.
5. Reinspect ownership and semantic task configuration after stop.
6. Release the instance proof guard immediately before the internal `RunEx`
   path, then wait for exact readiness.
7. If an old UUID existed, require the new UUID to differ. Reject the old UUID
   with `MCP_SERVICE_RESTART_FAILED`.
8. Report `started` when the service was already stopped and `restarted` when a
   prior active instance was stopped.
9. Preserve installed registration and observed runtime when start fails after a
   successful stop; do not attempt rollback to the old process.

**Tests**

- Restart holds one lifecycle operation without nested lock acquisition.
- Running service produces a different ready UUID and returns `restarted`.
- Stopped service runs only the start phase and returns `started`.
- Stop failure prevents `RunEx`.
- Same UUID after start returns restart failure.
- Drift before stop performs no mutation; drift after stop prevents start.
- Start timeout after stop leaves the task installed and retryable.

## Task 6: Implement conditional uninstall and semantic cleanup

**Files**

- Modify: `src/mcp_service/store.rs`
- Modify: `src/mcp_service/paths.rs` only for reusable containment checks
- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/model.rs`
- Modify: `src/mcp_service/output.rs`

**Implementation**

1. Add idempotent fixed-file removal helpers that distinguish missing from real
   write failures and never follow symlinks or recursively delete directories.
2. Build the definition deletion set before removing runtime/current metadata.
   Include only:
   - canonical managed definition paths referenced by a valid owned marker or
     valid current pointer;
   - the canonical runtime-derived path when its service identity matches a
     trusted marker/pointer;
   - fully valid same-service definitions with current user SID, canonical
     runtime/lock paths, and matching UUID filename.
3. Require every deletion candidate to normalize exactly under
   `<managed base>/definitions/<configuration UUID>.json`. Preserve external,
   unreferenced malformed, newer-schema, and unexpected files.
4. Add an operation success policy. Normal commands persist `completed`;
   successful uninstall removes `lifecycle.json` last while the lifecycle lease
   is still held. Failed uninstall preserves a failed lifecycle record where
   possible.
5. Implement `uninstall_locked`:
   - reject a foreign task before any mutation;
   - invoke `stop_locked(AllowExactOrphan)`;
   - retain the instance proof guard;
   - conditionally delete an owned task and confirm a subsequent inspect is
     missing;
   - remove runtime, verified definitions, current, and lifecycle through the
     success policy;
   - preserve both lock anchors and every excluded user artifact.
6. When the task is already missing, allow exact orphan control shutdown but no
   Scheduler fallback. Block cleanup if the lease is occupied without exact
   identity.
7. Return `already_uninstalled` only when task and removable metadata were
   already absent. Partial stale metadata cleanup returns `uninstalled` and
   `changed=true`.
8. Return `MCP_SERVICE_UNINSTALL_INCOMPLETE` after task deletion or cleanup has
   begun but the final state cannot be established. Do not recreate a deleted
   task.

**Tests**

- Running and stopped owned task uninstall.
- Already absent returns nullable identity fields and unchanged success.
- Missing task plus stale metadata converges to clean state.
- Exact live orphan receives control shutdown; unproven occupied orphan blocks
  cleanup.
- Foreign task preserves task and all metadata.
- Owned inactive safety-drifted task may be deleted; active drifted task cannot
  be force-stopped.
- Delete failure preserves metadata; post-delete cleanup failure remains
  retryable and never recreates the task.
- Failure injection after stop, delete, and each cleanup phase converges on the
  next uninstall.
- Referenced malformed definition may be removed only inside the canonical
  definitions directory.
- External paths, unreferenced malformed files, newer schemas, unexpected
  files, lock anchors, config, logs, binary, and plugins are preserved.

## Task 7: Integration coverage, documentation, and roadmap cleanup

**Files**

- Modify: `src/ai.rs`
- Modify: `tests/integration/ai.rs`
- Modify: `tests/integration/mcp.rs`
- Modify: `docs/reference/mcp.md`
- Modify: `docs/agents/recipes/mcp-http.md`
- Modify: `roadmap/managed-mcp-and-self-update.md`
- Modify implementation files only for fixes found by verification

**Implementation**

1. Add AI manual entries and examples for stop, restart, and uninstall.
2. Document idempotency, exact shutdown identity, forced Scheduler fallback,
   restart failure state, uninstall preservation guarantees, output schemas, and
   stable diagnostics.
3. Extend the agent recipe with safe status-before-mutation and recovery flows.
4. Add early-route integration tests proving all new lifecycle commands bypass
   invalid ambient configuration and dynamic plugin discovery.
5. Add process-level managed-runner coverage for controlled lease release and a
   new UUID after restart where this can be exercised without persistent Task
   Scheduler registration.
6. Do not register or delete a real persistent Windows task in normal workspace
   tests. Keep real Scheduler mutation behind an explicit opt-in/VM test.
7. Remove only the roadmap entries for stop, restart, and uninstall after the
   implementation and automated checks pass.
8. Keep restart/backoff, combined lifecycle/Windows VM tests, and target-client
   compatibility checks open.
9. Review the final diff and confirm the pre-existing modified
   `.agents/skills/release/SKILL.md` remains untouched and unstaged.

**Validation**

Run focused tests while iterating, then run:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked -- --test-threads=1
ah run check cargo build --locked
```

Also run:

- targeted managed-service unit and integration tests;
- the non-persistent typed Task Scheduler COM smoke test;
- `ah mcp service --help`;
- `ah ai info --domain mcp` or the equivalent host-command inspection;
- `git diff --check` and staged diff review.

If the default parallel workspace test again stalls on shared test resources,
report it separately; the deterministic sequential workspace run remains the
required complete result for this block.

## Commit strategy

The approved design is committed as `966cb25`. Commit this implementation plan
as a documentation step. Then implement Tasks 1-7 as one coherent roadmap
block, validate the complete behavior and diff, remove the three demonstrated
roadmap entries, and create one feature commit so code, tests, documentation,
and roadmap cleanup remain inseparable.
