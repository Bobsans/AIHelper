# Managed MCP Lifecycle Component Tests Implementation Plan

## Status

Approved design. Ready for implementation.

## Goal

Complete the automated managed MCP lifecycle orchestration matrix with
in-process component integration tests. The tests use real temporary durable
state and file leases behind scripted Scheduler and runtime boundaries. They do
not create persistent Windows Task Scheduler tasks and do not change production
behavior or public APIs.

The approved design is recorded in
`docs/plans/2026-07-22-mcp-managed-lifecycle-component-tests-design.md`.

## Working rules

- Preserve `.agents/skills/release/SKILL.md` and any other user changes.
- Keep production code unchanged except for replacing the inline test module
  body with `#[cfg(test)] mod tests;`.
- Use deterministic identities and typed event assertions instead of snapshots
  containing timestamps or random UUID values.
- Establish concurrency ordering with synchronization primitives, never sleeps.
- Run the smallest relevant test after each red/green edit.
- Commit each completed task separately after reviewing the diff.
- Do not change the roadmap until the complete automated matrix passes.
- Leave real CLI and persistent Task Scheduler checks for the Windows VM/manual
  roadmap item.

## Task 1: Mechanically relocate the existing lifecycle tests

Files:

- Modify `src/mcp_service/lifecycle.rs`.
- Create `src/mcp_service/lifecycle/tests/mod.rs`.
- Create `src/mcp_service/lifecycle/tests/harness.rs`.
- Create `src/mcp_service/lifecycle/tests/reducer.rs`.
- Create `src/mcp_service/lifecycle/tests/install_start_status.rs`.
- Create `src/mcp_service/lifecycle/tests/stop_restart.rs`.
- Create `src/mcp_service/lifecycle/tests/uninstall_recovery.rs`.
- Create `src/mcp_service/lifecycle/tests/concurrency.rs`.

Steps:

1. Replace the inline `mod tests { ... }` in `lifecycle.rs` with the external
   descendant declaration `mod tests;` under the existing `#[cfg(test)]`.
2. Move `FakeScheduler`, `FakeReadiness`, and shared builders/assertion helpers
   to `harness.rs` without changing behavior.
3. Move the two runtime reducer tests and their narrow evidence builders to
   `reducer.rs`.
4. Move install, start, and status cases to `install_start_status.rs`.
5. Move stop and restart cases to `stop_restart.rs`.
6. Move uninstall and recovery cases to `uninstall_recovery.rs`.
7. Leave `concurrency.rs` declared but empty until the deterministic harness
   gate is available.
8. Keep Windows-only orchestration cases under `#[cfg(windows)]`; keep pure
   reducer cases portable.
9. Run:

   ```text
   cargo fmt --all -- --check
   cargo test --locked --lib mcp_service::lifecycle::tests
   ```

10. Review the diff and confirm it contains only test relocation and import/path
    adjustments. Commit as `test: split lifecycle test modules`.

## Task 2: Build the stateful scripted lifecycle harness

Files:

- Modify `src/mcp_service/lifecycle/tests/harness.rs`.
- Modify scenario modules only when a moved test needs a compatibility update.

Steps:

1. Replace counter-only fake state with cloneable shared handles around a
   `ScriptedScheduler` state and a `ScriptedRuntimeControl` state.
2. Add typed scheduler events for meaningful external boundaries:

   - inspect;
   - register;
   - run;
   - enumerate instances;
   - stop exact instance;
   - delete owned task.

3. Add typed runtime events for readiness inspection and identity-aware
   shutdown.
4. Add one-shot fault queues for inspect, register/readback, run, instances,
   stop, delete, and shutdown. Allow registration to return an explicit drifted
   observation independently from an adapter error.
5. Model only adapter-contract state transitions: successful registration
   publishes an owned observation, successful exact stop removes the selected
   instance, and successful delete makes the task missing.
6. Add a `LifecycleHarness` that owns one `TempDir`, real `ServicePaths`, real
   `ServiceStore` through `LifecycleService`, scripted adapter handles, fixed
   identities, and short bounded polling timeouts.
7. Add builders for installed stopped state, installed ready state, owned and
   foreign task observations, runtime documents, readiness queues, live instance
   leases, and preserved sentinel artifacts.
8. Add invariant helpers for adapter events, forbidden destructive events,
   current/definition/runtime/lifecycle documents, and exact file bytes.
9. Add a deterministic run gate that reports when `run` is entered and blocks
   until explicitly released. Use channels, barriers, or condition variables;
   bounded waits may fail a broken test but must not establish ordering.
10. Keep the existing relocated tests green while changing their setup to the
    new harness.
11. Run:

    ```text
    cargo fmt --all -- --check
    cargo test --locked --lib mcp_service::lifecycle::tests
    ```

12. Review for duplicated production decision logic. Commit as
    `test: add lifecycle scenario harness`.

## Task 3: Complete install, start, and status coverage

Files:

- Modify `src/mcp_service/lifecycle/tests/install_start_status.rs`.
- Modify `src/mcp_service/lifecycle/tests/harness.rs` only for evidence-oriented
  helpers required by these cases.

Steps:

1. Add a default-install case proving verified registration, current-pointer
   publication, one run request, and success only after exact readiness.
2. Add a repeated-ready install case proving no additional register or run.
3. Add a configuration-update case proving stable service ID, replacement
   configuration ID, exact old-instance stop before activation, and replacement
   readiness.
4. Add a `--no-start` configuration-update case proving the live old instance
   is neither stopped nor replaced by a run request.
5. Add registration error and drifted-readback cases proving the candidate
   current pointer is not published and a later retry converges.
6. Add exact-ready start idempotency and run-failure recovery cases.
7. Add foreign-task and semantic-drift cases proving no unsafe run.
8. Expand status cases across stopped, ready, running-not-ready, failed,
   identity mismatch, configuration drift, Scheduler error, and invalid durable
   documents.
9. Prove status diagnostics and drift entries are deterministic and sorted.
10. Capture current, definition, runtime, and lifecycle bytes before status and
    prove they remain unchanged afterward.
11. Hold the lifecycle lease and prove status returns busy operation evidence
    without waiting.
12. For each failure case, assert the stable diagnostic, allowed/forbidden
    mutations, and retryable durable state.
13. Run:

    ```text
    cargo fmt --all -- --check
    cargo test --locked --lib mcp_service::lifecycle::tests::install_start_status
    ```

14. Review the diff and commit as
    `test: cover lifecycle install start status`.

## Task 4: Complete stop and restart coverage

Files:

- Modify `src/mcp_service/lifecycle/tests/stop_restart.rs`.
- Modify `src/mcp_service/lifecycle/tests/harness.rs` only for exact stop/restart
  evidence required by these cases.

Steps:

1. Add control failure coverage proving fallback targets only the exact,
   revalidated Scheduler instance.
2. Add Scheduler stop failure and retry coverage proving registration and
   durable identity evidence survive the failed attempt.
3. Add restart of an already stopped service and require a `started` result.
4. Add restart stop-failure coverage proving no run request occurs.
5. Add start failure after a successful restart stop, require honest
   stopped/failed durable state, then prove a later start converges.
6. Add post-stop task drift coverage proving restart does not run the drifted
   task.
7. Preserve the relocated exact-identity, unsafe PID/drift, timeout, and reused
   instance UUID cases.
8. Assert order only for stop-old before run-new; avoid exact polling-count
   assertions.
9. Run:

   ```text
   cargo fmt --all -- --check
   cargo test --locked --lib mcp_service::lifecycle::tests::stop_restart
   ```

10. Review the diff and commit as `test: cover lifecycle stop restart`.

## Task 5: Complete uninstall, recovery, and concurrency coverage

Files:

- Modify `src/mcp_service/lifecycle/tests/uninstall_recovery.rs`.
- Modify `src/mcp_service/lifecycle/tests/concurrency.rs`.
- Modify `src/mcp_service/lifecycle/tests/harness.rs` only for reusable evidence
  required by these cases.

Steps:

1. Add running-service uninstall coverage proving exact stop, owned task delete,
   and verified semantic metadata cleanup in that order.
2. Create executable/configuration/log/plugin/unexpected sentinel artifacts and
   prove uninstall preserves them together with lock anchors.
3. Add owned-task delete failure and retry coverage proving cleanup does not
   begin before successful deletion.
4. Add missing-task recovery from stale or partially removed metadata.
5. Add occupied unsafe-orphan coverage proving cleanup is blocked without exact
   identity.
6. Add foreign-task coverage proving no stop, delete, or ownership inference.
7. Preserve malformed, newer-schema, idempotent, and already-uninstalled cases.
8. Add the representative concurrency scenario:

   - start restart on a background thread;
   - block the scripted run boundary after restart's stop phase;
   - require status to report `busy` with `operation=restart` immediately;
   - require a second mutator to return `MCP_SERVICE_BUSY` with no extra mutation;
   - release run and require exact readiness and successful restart completion.

9. Use bounded channel/barrier waits and join every spawned thread.
10. Run:

    ```text
    cargo fmt --all -- --check
    cargo test --locked --lib mcp_service::lifecycle::tests::uninstall_recovery
    cargo test --locked --lib mcp_service::lifecycle::tests::concurrency
    cargo test --locked --lib mcp_service::lifecycle::tests
    ```

11. Review the diff and commit as
    `test: cover lifecycle recovery concurrency`.

## Task 6: Validate the automated matrix and update the roadmap

Files:

- Modify `roadmap/managed-mcp-and-self-update.md`.
- Do not modify production sources unless a test exposes an actual production
  defect; if that happens, stop and design the behavior change separately.

Steps:

1. Run the existing managed-process integration tests and the non-persistent COM
   smoke identified by the current test suite.
2. Run the required repository checks:

   ```text
   ah run check cargo fmt --all -- --check
   ah run check cargo test --workspace --all-targets --locked
   ah run check cargo build --locked
   ```

3. If the `ah run check` transport deadline expires, run the exact command
   directly with a larger process timeout and record the fallback.
4. Review all changed test files for persistent task creation, hidden public test
   APIs, nondeterministic sleeps, brittle polling counts, and changes to released
   JSON fields or plugin ABI. None are allowed.
5. Remove this completed combined roadmap item:

   ```text
   - [ ] Добавить unit, integration и Windows VM tests для lifecycle-команд.
   ```

6. Insert the remaining work as one explicit open item:

   ```text
   - [ ] Выполнить Windows VM/manual end-to-end lifecycle matrix через реальный
         ah.exe и persistent per-user Task Scheduler task.
   ```

7. Confirm the roadmap contains no completed checkbox for this block and still
   preserves the detailed managed-service verification matrix.
8. Record the completed automated component-test milestone in the `aihelper`
   basic-memory project without claiming CLI, persistent Task Scheduler, or VM
   coverage.
9. Review and commit only the roadmap update as
   `docs: update lifecycle test roadmap`.

## Completion criteria

- The old inline lifecycle tests are organized under the private descendant
  test tree and remain green.
- The scripted harness uses real temporary persistence and leases while faking
  only Scheduler and runtime boundaries.
- The approved install/start/status, stop/restart, uninstall/recovery, and
  concurrency scenarios pass deterministically.
- No persistent Task Scheduler task is created by the automated suite.
- Production behavior, public API, JSON schema, and plugin ABI are unchanged.
- Required workspace checks pass, or an exact documented fallback passes after
  an `ah` wrapper timeout.
- The combined roadmap item is removed and only the real Windows VM/manual E2E
  item remains open.
