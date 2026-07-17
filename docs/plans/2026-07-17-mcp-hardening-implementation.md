# MCP Runtime Hardening Implementation Plan

**Date:** 2026-07-17
**Design:** `docs/plans/2026-07-17-mcp-hardening-design.md`

## 1. Execution coordinator

- Replace split atomic/mutex lifecycle tracking in
  `crates/ah-runtime/src/executor.rs` with one coordinator state.
- Add unique execution generations, prompt queued cancellation/deadlines, and
  retryable draining admission errors.
- Remove tracked entries before ordinary completion delivery and make late
  worker cleanup generation-safe.
- Add deterministic executor tests for transition races, draining, recovery,
  queued deadlines, panic cleanup, and ID reuse.

## 2. MCP request identity

- Generate unique execution IDs in `crates/ah-mcp/src/server.rs`.
- Maintain protocol-to-execution cancellation mappings with scoped cleanup.
- Map draining failures to a stable retryable diagnostic.
- Extend MCP tests for reused protocol IDs and cancellation mapping cleanup.

## 3. Cooperative cancellation scopes

- Replace entry-time cancellation deletion with RAII cleanup in `run`, `task`,
  and `search`.
- Apply the same lifecycle to GitHub and GitLab typed adapters.
- Check pre-delivered cancellation before command work.
- Add regression tests for cancellation delivered before handler entry and
  panic-safe cleanup where practical.

## 4. Search correctness and allocation

- Correct context-before indexing.
- Scan borrowed lines and apply the remaining global limit inside each file.
- Use one extra match to calculate `truncated` exactly.
- Poll cancellation during line scanning.
- Add context, truncation, and bounded-result regression tests.

## 5. Typed registry and schema validation

- Introduce a cached immutable typed registry in `ah-runtime` with compiled
  input/output validators and deterministic command lookup.
- Invalidate definition state on registration/discovery and version enabled
  state only on real disabled-domain changes.
- Enforce the MCP-compatible input root-schema subset.
- Expose cheap enabled-command lookup and catalog revision APIs.
- Add registry build-count, schema compatibility, and revision tests.

## 6. MCP catalog snapshot

- Replace per-call command enumeration and serialized fingerprints with a
  revisioned `Arc` snapshot containing tools and name lookup.
- Refresh and notify only when the runtime revision changes.
- Verify complete old/new snapshots under concurrent reads.

## 7. Reserved host domains

- Let the application reserve dynamic domains before discovery.
- Reserve `ai`, `plugins`, and `mcp` in startup flow.
- Skip conflicting dynamic plugins with deterministic diagnostics.
- Add loader and routing regressions.

## 8. Transactional persistence

- Add a shared bounded sidecar-lock and atomic JSON replacement helper.
- Route CLI and MCP plugin-setting mutations through clone/save/publish.
- Route task-store read-modify-write through per-path and cross-process locks.
- Add save-failure, concurrent-update, parseability, and lock-timeout tests.

## 9. Documentation and validation

- Update MCP/plugin developer documentation for draining, schema restrictions,
  reserved domains, and persistence guarantees.
- Run focused crate/domain tests while iterating.
- Run `cargo fmt`, workspace tests, Clippy, debug build, and release build via
  `ah run check` before handoff.
