# MCP Managed Service Foundation Implementation Plan

## Goal

Implement the first usable Windows managed-service slice from Stage 2 of the
roadmap: durable per-user configuration, single-instance ownership, direct Task
Scheduler 2.0 registration, early lifecycle routing, and the `install`, `start`,
and `status` commands.

The normative behavior and JSON contracts are defined in
`2026-07-22-mcp-managed-service-foundation-design.md`. This plan only sequences
the work and identifies the tests that make each layer safe to compose.

## Completion boundary

This block is complete when:

- `ah mcp service install [--no-start]`, `start`, and `status` work on Windows;
- lifecycle commands and managed serve preflight run before plugin discovery;
- a registered task can only start one managed instance for the current user;
- definitions and runtime state survive process restarts and partial writes;
- task drift and lifecycle diagnostics are deterministic in text and JSON;
- non-Windows builds keep manual MCP behavior and return a stable unsupported
  diagnostic for managed-service commands;
- relevant unit and integration tests, workspace format, test, and build checks
  pass;
- completed roadmap entries are removed, while real Scheduler crash/restart,
  combined Windows VM lifecycle, and client compatibility checks remain open.

## Task 1: Establish the managed-service domain model

**Files**

- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `src/lib.rs`
- Create: `src/mcp_service/mod.rs`
- Create: `src/mcp_service/model.rs`
- Create: `src/mcp_service/paths.rs`
- Create: `src/mcp_service/output.rs`

**Implementation**

1. Add root `uuid` support and Windows-only `windows` crate features required
   for COM, Task Scheduler, OLE, and variants. Retain `windows-sys` for existing
   low-level process and lock APIs.
2. Add typed models for the immutable service definition, current pointer,
   runtime state, lifecycle state, task marker, drift entry, status result, and
   mutation result. Use the exact field names and nullability from the design.
3. Centralize format validation for UUIDs, RFC 3339 UTC timestamps, decimal
   PIDs, versions, ports, HRESULTs, and lifecycle phase invariants.
4. Implement Windows path identity normalization and comparison without
   environment-variable expansion. Keep path construction injectable through
   `ServicePaths` so tests never write to real LocalAppData.
5. Add deterministic text renderers and JSON serialization helpers.

**Tests**

- Round-trip every persisted and emitted schema.
- Reject malformed identifiers, timestamps, PIDs, and illegal state-field
  combinations.
- Cover separator, trailing-separator, absolute/canonical, and case-insensitive
  Windows path identity behavior.
- Snapshot representative installed, not-installed, drifted, and scheduler-error
  outputs.

## Task 2: Implement durable storage and leases

**Files**

- Create: `src/mcp_service/store.rs`
- Create: `src/mcp_service/lock.rs`
- Modify: `src/mcp_service/mod.rs`

**Implementation**

1. Persist immutable definitions using create-new semantics, flush the file,
   reopen it, deserialize it, and compare it to the intended value before use.
2. Write mutable pointer and state files atomically. Readers must classify
   absent, valid, malformed, and unsupported-newer documents without mutating
   them.
3. Implement the task-authority reconciliation matrix: repair only a missing or
   stale current pointer when the scheduler marker identifies an exact valid
   definition; never overwrite malformed or newer data.
4. Add exclusive `CreateFileW`-handle leases for lifecycle mutation and managed
   instance ownership. Lifecycle mutation uses a bounded wait; status probes the
   lifecycle lock once and never waits.
5. Expose platform-neutral traits/fakes where unit tests need to model lock
   contention and state transitions. Non-Windows production code returns the
   stable unsupported diagnostic.

**Tests**

- Verify immutable idempotency and conflicting-content rejection.
- Simulate partial/malformed/current-newer documents and every reconciliation
  matrix branch.
- Verify lifecycle lock timeout and nonblocking status contention reporting.
- Verify a second instance lease fails while the first handle remains open and
  succeeds after it closes.

## Task 3: Add managed runner hooks without breaking manual serve

**Files**

- Modify: `crates/ah-mcp/src/server.rs`
- Modify: `crates/ah-mcp/src/lib.rs`
- Modify: `src/cli.rs`
- Modify: `src/runtime_flow.rs`
- Create: `src/mcp_service/runner.rs`
- Modify: `src/mcp_service/mod.rs`

**Implementation**

1. Extend `ah-mcp` with an opt-in managed HTTP entry point that accepts a fixed
   instance ID and a listener-bound callback. Preserve all existing public
   function signatures and manual serve behavior.
2. Add a minimal raw-argv early router before event logging, configuration
   startup, built-in registration, and dynamic plugin discovery. It recognizes
   all `mcp service` commands and `mcp serve --managed-config` while leaving
   ordinary commands on the existing parser path.
3. For managed serve, load and validate the exact immutable definition, acquire
   the instance lease before discovery, write `starting`, and configure the
   event logger from the managed definition.
4. Pass one fixed instance ID into `ah-mcp`; after listener bind, atomically
   write the exact `ready` runtime state. On orderly shutdown write `stopping`
   then `stopped`; on fatal startup/runtime failure write `failed` with the
   specified exit information where possible.

**Tests**

- Prove lifecycle and managed-serve argv reach the early router without plugin
  discovery or ordinary startup side effects.
- Prove unrelated and manual serve argv remain on the existing path.
- Prove the callback observes the bound listener and the same fixed instance ID
  returned by readiness.
- Prove a second managed runner exits with `MCP_SERVICE_ALREADY_RUNNING`.
- Preserve all existing MCP serve, shutdown, readiness, and fatal-exit tests.

## Task 4: Implement Scheduler adapter and semantic drift

**Files**

- Create: `src/mcp_service/scheduler.rs`
- Create: `src/mcp_service/windows_scheduler.rs`
- Modify: `src/mcp_service/mod.rs`

**Implementation**

1. Define a narrow scheduler port covering inspect, register/update, run, and
   the state required by this block. Use an in-memory fake for lifecycle tests.
2. On Windows, initialize COM and use typed Task Scheduler 2.0 interfaces only.
   Register the task at the root with current-user interactive-token/LUA
   security, logon trigger, exact exec action, working directory, and canonical
   settings from the design.
3. Store the exact ownership marker in registration info and refuse to take over
   unowned or ambiguously owned tasks.
4. Convert scheduler HRESULTs and state into stable diagnostics without leaking
   locale-dependent COM messages into deterministic JSON fields.
5. Inspect task properties semantically and emit sorted drift entries by stable
   property key. Do not compare exported XML.

**Tests**

- Fake-adapter tests for absent, exact, drifted, foreign, disabled, running, and
  scheduler-error tasks.
- Property-level tests for every canonical trigger, action, principal, and
  setting, including restart count 3 and interval `PT1M`.
- Windows-only smoke test that COM interfaces can be constructed and task
  definitions can be populated without registering a persistent task.

## Task 5: Implement install, start, and status orchestration

**Files**

- Create: `src/mcp_service/lifecycle.rs`
- Modify: `src/mcp_service/output.rs`
- Modify: `src/mcp_service/mod.rs`
- Modify: `src/runtime_flow.rs`
- Modify: `tests/integration/mcp.rs`

**Implementation**

1. `install` acquires the lifecycle lease, derives stable service identity,
   creates or reuses an immutable definition, registers or updates only an owned
   task, reconciles the current pointer, and starts by default.
2. `install --no-start` performs no stop/restart side effects. A changed
   configuration may request control shutdown only for the exact old ready
   instance; unsafe replacement returns `MCP_SERVICE_RESTART_REQUIRED`.
3. `start` validates registration and durable state, calls `RunEx`, and polls for
   up to 15 seconds until runtime and readiness agree on configuration ID,
   instance ID, PID, version, and endpoint. A successful `RunEx` alone is never
   reported as success.
4. `status` reads registration, scheduler, runtime, readiness, lifecycle, and
   drift independently. Reduce them with the precedence from the design and
   return exit code 0 for not-installed, drift, and scheduler-error states.
5. Emit the exact mutation and status JSON schemas; text output conveys the same
   state without unstable platform prose.

**Tests**

- Table-driven lifecycle tests for fresh install, idempotent install, no-start,
  configuration change, foreign task conflict, start readiness success, start
  timeout, stale readiness, malformed state, drift, scheduler failure, and lock
  contention.
- CLI integration tests for text/JSON contracts, stable errors, unsupported
  platforms, and the guaranteed status exit code.
- Assert lifecycle operations do not discover or load dynamic plugin DLLs.

## Task 6: Documentation, roadmap cleanup, and verification

**Files**

- Modify: `docs/reference/mcp.md`
- Modify: `docs/agents/recipes/mcp-http.md`
- Modify: `roadmap/managed-mcp-and-self-update.md`
- Modify implementation files only for fixes found by verification

**Implementation**

1. Document Windows prerequisites, install/start/status examples, `--no-start`,
   storage paths, JSON contracts, drift semantics, and recovery diagnostics.
2. Extend the AI recipe with agent-safe inspection and idempotent install flow.
3. Remove only roadmap entries demonstrated by automated checks and the
   implemented code. Keep restart/backoff open until a real Scheduler crash
   test passes; keep the combined Windows VM lifecycle and deferred client
   compatibility checks open.
4. Review the final diff and confirm the pre-existing modified release skill is
   still untouched and unstaged.

**Validation**

Run in order:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

Also run focused managed-service tests while iterating and report any Windows
Scheduler smoke check that cannot run in the current environment.

## Commit strategy

The approved design is already committed. Commit this implementation plan as a
documentation step. Then implement Tasks 1-6 as one coherent roadmap block,
verify the complete diff, and create one implementation commit so the roadmap
cleanup cannot become separated from the behavior and tests that justify it.
