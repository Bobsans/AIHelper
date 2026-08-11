# Invocation Diagnostics Hardening Implementation Plan

## Objective

Implement the approved diagnostics-hardening design from
`docs/plans/2026-08-10-invocation-diagnostics-hardening-design.md` while preserving released command JSON, plugin ABI compatibility, deterministic output, and all pre-existing working-tree changes. The completed package must make ctx traversal resilient to full-read invalid UTF-8, retain partial managed-service diagnostics after scheduler failure, record an allowlisted `run.check` outcome for CLI and MCP completion events, and isolate integration-test config/log state.

## Workstreams

### Stream 1 – ctx full-read classification

- Add a strict classified-read helper in `src/commands/ctx/io.rs`.
  - Return valid content separately from invalid UTF-8.
  - Preserve every non-`InvalidData` I/O failure as `AppError`.
  - Definition of Done: no lossy decode and no change to the shared prefix sniff.
- Update `process_pack_entry` and `collect_symbols_for_file` in `src/commands/ctx/domain.rs`.
  - Reuse existing binary-skip accounting.
  - Preserve pack and symbols output semantics.
- Add regression coverage in `tests/integration/ctx.rs`.
  - Invalid byte occurs after more than 8192 valid bytes.
  - Include a valid neighboring source file.
  - Cover a valid multibyte boundary case where practical.
- Validation:
  - `ah run check cargo test --test integration ctx --locked`
  - Focused ctx unit tests if the helper receives local tests.

### Stream 2 – managed-service partial status

- Refactor `status_snapshot` and runtime observation in `src/mcp_service/lifecycle.rs`.
  - Record scheduler error/HRESULT without returning early.
  - Continue only through the validated current-pointer and canonical-definition chain.
  - Pass explicit `SchedulerRuntimeEvidence` to runtime reduction.
  - Prevent later registration/drift finalization from overwriting `scheduler_error`.
- Extend the existing lifecycle harness/tests under
  `src/mcp_service/lifecycle/tests/`.
  - Scheduler failure plus valid runtime/readiness reports partial healthy evidence.
  - Invalid definition keeps readiness `not_checked`.
  - Durable state bytes are unchanged.
- Preserve current concurrent changes in lifecycle, readiness, scheduler, runner, and path handling as authoritative.
- Validation:
  - `ah run check cargo test mcp_service::lifecycle --locked`
  - Existing install/start/status and recovery suites.

### Stream 3 – safe run.check event outcome

- Add an internal observation result in `crates/ah-runtime/src/lib.rs`.
  - Existing `BuiltinPlugin::invoke` remains unchanged.
  - A default observed-invocation method delegates to `invoke` with no outcome.
  - Dynamic plugins remain on the existing path and produce no outcome.
  - `PluginManager::invoke_observed` performs the same disabled-domain and required-tool checks as `invoke`.
- Add the typed three-field run outcome in the internal runtime observation model.
  - Fields: `success`, `timed_out`, and nullable `exit_code`.
  - Do not modify `ah-plugin-api` request/response types or C ABI payloads.
- Update `src/commands/run.rs` and `RunBuiltinPlugin` in `src/plugins.rs`.
  - Emit the normal CLI result and return the safe observation from the same `RunCheckOutput`.
  - Avoid parsing rendered stdout and avoid thread-local last-result state.
- Update CLI execution/logging in `src/runtime_flow.rs` and `src/event_log.rs`.
  - Thread the optional observation to `record_cli_command`.
  - Preserve outer success/error semantics.
  - Serialize only the allowlisted outcome fields.
- Update MCP events in `crates/ah-mcp/src/server.rs` and `crates/ah-mcp/src/jobs.rs`.
  - Extract the same projection only for canonical `run.check` successful typed results.
  - Capture detached-job outcome before moving the full response into job storage.
  - Leave wrapper error/cancellation events without outcome.
- Update event tests in `crates/ah-mcp/src/server.rs`, `crates/ah-mcp/src/events.rs`, and `src/event_log.rs`, plus integration logging/MCP coverage.
- Update `docs/reference/logging.md` and `docs/developers/logging.md`; update `docs/reference/run.md` only if the log contract is described there.
- Validation:
  - `ah run check cargo test -p ah-runtime --locked`
  - `ah run check cargo test -p ah-mcp --locked`
  - `ah run check cargo test --test integration logging --locked`
  - `ah run check cargo test --test integration mcp --locked`
  - `ah run check cargo test --test integration run --locked`

### Stream 4 – integration-test process isolation

- Add owned fixtures to `tests/integration/common.rs`.
  - One wrapper for `assert_cmd::Command`.
  - One guard/factory for long-lived `std::process::Command` children.
  - Each fixture owns a unique `TempDir` until command completion.
- Migrate ordinary integration launches in `ai.rs`, `ctx.rs`, `file.rs`, `git.rs`, `help.rs`, `http.rs`, `mcp.rs`, `plugins.rs`, `project.rs`, `run.rs`, `search.rs`, and `task.rs`.
  - Preserve explicit config-directory tests and plugin-setting shared-directory tests.
  - Keep `logging.rs` on explicit directories.
  - Keep `upgrade.rs` empty-variable coverage explicit.
- Add fixture tests proving unique directories and lifetime through child wait.
- Add a source contract check or exact-search assertion that raw `cargo_bin("ah")` remains only in intentional locations.
- Validation:
  - `ah run check cargo test --test integration --locked`
  - Exact search for remaining raw `cargo_bin("ah")` sites.

### Stream 5 – documentation and full validation

- Reconcile documentation examples with the final serialized event shape.
- Run formatting and focused checks after each stream.
- Run repository handoff checks:
  - `ah run check cargo fmt --all -- --check`
  - `ah run check cargo test --workspace --all-targets --locked`
  - `ah run check cargo build --locked`
- Inspect final diff and status.
  - Confirm no generated or unrelated files changed.
  - Confirm pre-existing user changes were preserved.
  - Confirm plugin API/ABI files were not modified for the outcome channel.

## Execution Order

1. Implement and validate ctx classification.
2. Implement and validate lifecycle partial status while adapting to the current dirty lifecycle files.
3. Implement the internal runtime observation and CLI logging path.
4. Implement MCP sync/job outcome propagation and event serialization tests.
5. Add integration-test isolation and migrate call sites.
6. Update documentation and run full validation.

## Risks and Mitigations

- **Dirty lifecycle/runtime files overlap the work.** Re-read each file immediately before patching and preserve the current content as authoritative.
- **Observation API accidentally changes stable plugin contracts.** Keep the model in `ah-runtime`; do not modify `ah-plugin-api` or dynamic-plugin JSON.
- **Scheduler error is overwritten after partial observation.** Use an explicit partial branch and assert the final registration value.
- **Outcome leaks output or secrets.** Construct it from typed scalar fields; never accept arbitrary JSON as an event outcome.
- **Detached jobs lose the result before logging.** Extract the outcome before response ownership moves to the registry.
- **Test fixtures drop config directories too early.** Store the `TempDir` inside the wrapper/child guard and test its lifetime.
- **Mechanical test migration changes intentional config tests.** Exclude logging, empty-variable, and shared plugin-setting cases from automatic replacement.

## Rollback

Each stream is additive and can be reverted independently. If the internal observation API causes unexpected compatibility issues, remove `invoke_observed` and its event plumbing while retaining ctx, lifecycle, and test-isolation fixes. No persistent user data migration is introduced.
