# MCP Managed Restart Policy Implementation Plan

## Goal

Complete the first-version bounded managed MCP restart policy defined in
`2026-07-22-mcp-managed-restart-policy-design.md` without adding a second retry
controller or persistent Scheduler mutations to automated tests.

## Completion boundary

This block is complete when:

- the canonical Task Scheduler restart policy is named and fixed at three
  retries with interval `PT1M`;
- both restart properties are written, read back, and compared for drift;
- `restart_backoff` requires conservative Scheduler and durable managed failure
  evidence;
- fatal managed process exit and clean shutdown behavior remain proven;
- command reference and AI recipe describe the policy and inference boundary;
- the implementation roadmap entry is removed while the Windows VM item stays
  open with an explicit operational matrix;
- focused and workspace checks pass.

## Task 1: Centralize and verify the canonical policy

**Files**

- Modify: `src/mcp_service/scheduler.rs`

**Implementation**

1. Add public crate-level constants for restart count `3` and interval `PT1M`.
2. Use the constants in `DesiredTaskSpec::canonical`.
3. Add a small predicate that checks only the observed restart settings against
   the canonical policy. Other semantic drift remains independent.
4. Keep the released task schema and every existing field unchanged.

**Tests**

- Assert the canonical spec uses both constants.
- Detect restart-count and restart-interval drift independently.
- Assert multiple drift entries remain deterministically sorted.

## Task 2: Make backoff reduction evidence-based

**Files**

- Modify: `src/mcp_service/lifecycle.rs`

**Implementation**

1. Introduce an internal typed Scheduler runtime-evidence value containing:
   Scheduler state, last result, and canonical restart-policy match.
2. Pass the full observed Scheduler evidence from status collection into the
   runtime reducer. Internal mutation-only observations use evidence that cannot
   infer backoff.
3. Derive `restart_backoff` only when:
   - Scheduler is queued;
   - the lifecycle is idle;
   - restart settings are canonical;
   - `LastTaskResult` is known and nonzero;
   - durable runtime is failed with a non-clean, nonzero last exit;
   - earlier reducer precedence has already ruled out identity mismatch,
     stopping, readiness, a running task, and an occupied instance lease.
4. Map remaining queued states to `starting` instead of guessing a restart
   reason.
5. Preserve the managed failure diagnostic for both `failed` and
   `restart_backoff`.
6. Do not add public fields, schema versions, or diagnostics.

**Tests**

- Queued canonical nonzero Scheduler result plus durable nonzero failure reduces
  to `restart_backoff`.
- Clean stopped runtime, zero/unknown Scheduler result, zero durable exit,
  policy drift, or busy lifecycle does not infer backoff.
- Remaining queued evidence reduces to `starting`.
- Exact readiness, occupied lease, and running Scheduler state retain their
  existing precedence.
- A status snapshot in inferred backoff retains the managed diagnostic and
  original Scheduler result.

## Task 3: Strengthen process and COM proof

**Files**

- Modify: `tests/integration/mcp.rs`
- Modify: `src/mcp_service/windows_scheduler.rs`

**Implementation**

1. Add a Windows process integration case that holds the configured loopback
   port, launches the managed runner, and proves a fast fatal exit.
2. Verify the durable runtime record is `failed`, its last exit is non-clean and
   nonzero, and its stable diagnostic identifies the server failure.
3. Keep the existing clean control-shutdown proof for exit zero and duplicate
   runner proof for safe success.
4. Extend the non-persistent COM smoke to read `RestartInterval` as well as
   `RestartCount` from the populated in-memory `ITaskDefinition`.
5. Continue to use `NewTask` only; never call `RegisterTaskDefinition`.

**Tests**

- Run the focused scheduler, lifecycle, runner, and Windows managed-process
  tests.
- Confirm the COM smoke reads exactly `3` and `PT1M`.

## Task 4: Document and close the implementation item

**Files**

- Modify: `docs/reference/mcp.md`
- Modify: `docs/agents/recipes/mcp-http.md`
- Modify: `roadmap/managed-mcp-and-self-update.md`

**Implementation**

1. Document three retries after the initial launch, one-minute Scheduler-owned
   spacing, and zero/nonzero exit behavior.
2. Explain that `restart_backoff` is inferred from combined evidence, not a
   native Windows countdown; no attempt number or next-retry timestamp is
   available.
3. Tell agents to inspect Scheduler result, runtime diagnostic, readiness, and
   restart-setting drift together.
4. Remove only the completed Stage 2 restart-policy checklist entry.
5. Keep the combined lifecycle/Windows VM tests open and expand their matrix to
   require:
   - no more than three launches after the initial failure;
   - intervals approximately matching `PT1M`;
   - no launch after the final retry;
   - no retry after clean exit;
   - observational recording of actual Task Scheduler states without requiring
     `TASK_STATE_QUEUED` as an acceptance condition.

## Validation and commit

Run focused checks while iterating, then:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

Also run the non-persistent Windows COM smoke and managed fatal-exit integration
test explicitly when useful for diagnosis. Do not perform the deferred
persistent-task or multi-version Windows VM checks in this block.

Review the final staged diff, preserve unrelated user changes, record the
completed milestone in the `aihelper` basic-memory project, and create one
focused implementation commit.
