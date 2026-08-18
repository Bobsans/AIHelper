# Windows Updater And Managed MCP Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix rollback lock handoff, disappearing Task Scheduler instance races, and bounded managed MCP restart behavior.

**Architecture:** Preserve the named-mutex lease introduced for the thin service launcher. Validate an inherited mutex by reopening the deterministic expected name and comparing kernel objects. Move bounded retry ownership into `ah-mcp-service.exe`, disable duplicate Scheduler retries, and derive backoff from a running launcher plus durable fatal runtime evidence.

**Tech Stack:** Rust 2024, `windows`/`windows-sys`, Windows Task Scheduler 2.0 COM, Cargo tests.

## Global Constraints

- Preserve released CLI, JSON, MCP, plugin ABI, and C ABI field names.
- Keep retry bounded to three retries after the initial launch with `PT1M` spacing.
- Preserve exact Task Scheduler ownership and process identity checks.
- Preserve unrelated HTTP worktree changes.
- Use test-first red/green cycles for every production change.

---

### Task 1: Named-mutex updater handoff

**Files:**
- Modify: `crates/ah-updater-core/src/installation.rs`
- Modify: `crates/ah-updater-core/src/lib.rs`
- Modify: `src/mcp_service/lock.rs`
- Modify: `crates/ah-update-helper/src/handoff.rs`

**Interfaces:**
- Produces: `ah_updater_core::lifecycle_mutex_name(path: &Path) -> String`
- Consumes: inherited named-mutex `HANDLE`, expected lifecycle-lock path, named ACK event.

- [x] **Step 1: Write failing tests**

Add a core test for deterministic mutex naming and replace the helper disk-handle test with a named-mutex claim test. The helper test must also reject a different named mutex.

- [x] **Step 2: Verify red**

Run: `ah run check cargo test -p ah-updater-core lifecycle_mutex_name --locked`

Run: `ah run check cargo test -p ah-update-helper handoff --locked`

Expected: compile/test failure because named-mutex validation is not implemented.

- [x] **Step 3: Implement the minimum fix**

Implement the shared name derivation, use it from `FileLease`, and validate the helper's inherited handle with `OpenMutexW` plus `CompareObjectHandles` before signaling the ACK event.

- [x] **Step 4: Verify green**

Run both targeted commands from Step 2 and require PASS.

### Task 2: Disappearing Scheduler instance race

**Files:**
- Modify: `src/mcp_service/windows_scheduler.rs`

**Interfaces:**
- Produces: `is_task_instance_gone(hresult: i32) -> bool`
- Consumes: `IRunningTask::State` and `IRunningTask::Stop` HRESULT `0x8004130B`
  (`SCHED_E_TASK_NOT_RUNNING`).

- [x] **Step 1: Write the failing test**

Extend HRESULT classification tests to require `0x8004130B` to be treated as an instance that disappeared during enumeration, while access and generic Scheduler failures remain errors.

- [x] **Step 2: Verify red**

Run: `ah run check cargo test --bin aihelper task_instance_gone_codes --locked`

Expected: FAIL because `0x8004130B` is not classified.

- [x] **Step 3: Implement the minimum fix**

When `IRunningTask::State` returns the disappearing-instance HRESULT, skip that
stale collection entry. Treat the exact target disappearing before or during
`Stop()` as already stopped, then retain the existing quiescence proof. Preserve
identity errors and every other HRESULT.

- [x] **Step 4: Verify green**

Run the targeted test and the managed lifecycle test module.

### Task 3: Launcher-owned bounded restart policy

**Files:**
- Modify: `src/bin/ah-mcp-service.rs`
- Modify: `src/mcp_service/scheduler.rs`
- Modify: `src/mcp_service/windows_scheduler.rs`
- Modify: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/lifecycle/tests/reducer.rs`
- Modify: `docs/reference/mcp.md`
- Modify: `docs/agents/recipes/mcp-http.md`

**Interfaces:**
- Produces: launcher policy of one initial attempt plus three retries spaced by `Duration::from_secs(60)`.
- Produces: canonical Task Scheduler registration with native retry disabled, preventing duplicate retry multiplication.
- Produces: `restart_backoff` while the launcher task remains running and durable runtime records a fatal child exit.

- [x] **Step 1: Write failing launcher tests**

Add closure-driven tests proving zero-exit stops immediately, three failures followed by success make four attempts and three delays, and four failures stop after four attempts.

- [x] **Step 2: Verify red**

Run: `ah run check cargo test --bin ah-mcp-service retry --locked`

Expected: compile failure because the retry runner does not exist.

- [x] **Step 3: Write failing Scheduler/reducer tests**

Require canonical Scheduler retry values `0` and empty interval, and require running Scheduler plus durable fatal runtime and no instance lease to reduce to `restart_backoff`.

- [x] **Step 4: Verify red**

Run: `ah run check cargo test --bin aihelper restart_policy --locked`

Expected: FAIL against the existing Scheduler-owned policy.

- [x] **Step 5: Implement the minimum fix**

Wrap one child launch in a four-attempt loop, sleep only between failed attempts, clear native Scheduler restart settings, and update the reducer before the generic running/not-ready branch.

- [x] **Step 6: Update documentation**

Document launcher-owned retry timing, the disabled native Scheduler retry, and the observable `restart_backoff` evidence without adding public fields.

- [x] **Step 7: Verify green**

Run the targeted launcher, Scheduler, and reducer tests and require PASS.

### Task 4: Validation and Windows acceptance

**Files:**
- Modify only if a test exposes a root-cause defect in the scoped files above.

**Interfaces:**
- Consumes: debug/release `ah.exe`, `ah-mcp-service.exe`, and `ah-update-helper.exe` built from this worktree.

- [x] **Step 1: Run focused tests**

Run all updater handoff and managed MCP lifecycle tests.

- [x] **Step 2: Run repository gates**

Run formatting, workspace tests, debug build, and release build through `ah run check`.

- [x] **Step 3: Run Windows component acceptance**

Use an isolated managed registration to confirm immediate start/stop convergence and one fatal bind failure followed by a new launcher attempt after approximately `PT1M`. Confirm uninstall leaves no task, worker, or listener.

- [x] **Step 4: Inspect the final diff**

Confirm only this plan and the scoped updater/MCP/tests/docs files changed in addition to the authoritative pre-existing HTTP changes.

- [x] **Step 5: Record the milestone**

Append exact checks, remaining signed-release limitation, and final machine state to the `aihelper` basic-memory acceptance note.
