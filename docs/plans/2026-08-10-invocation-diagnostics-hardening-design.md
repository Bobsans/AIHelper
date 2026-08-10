# Invocation Diagnostics Hardening Design

**Date:** 2026-08-10  
**Status:** Approved

## Context

Recent command logs exposed four related reliability gaps:

1. `ctx.pack` and `ctx.symbols` can abort a whole traversal when a file passes the prefix binary sniff but fails strict UTF-8 decoding during the full read.
2. `mcp service status` returns immediately when Task Scheduler inspection fails, hiding independently observable runtime and readiness state.
3. `run.check` correctly reports a successful AIHelper invocation when the child exits non-zero, but the command log does not retain a safe child-process outcome.
4. Integration tests that do not isolate `AH_CONFIG_DIR` can write expected negative-test failures into the user's normal log directory.

The fixes must preserve released command JSON field names, plugin ABI compatibility, deterministic output, and the existing distinction between invocation success and child-process success.

## Goals

- Treat full-read invalid UTF-8 as a binary-file skip in `ctx.pack` and `ctx.symbols`.
- Preserve scheduler diagnostics while continuing best-effort runtime and readiness observation.
- Add a minimal, allowlisted `run.check` outcome to command-completion logs.
- Isolate every non-logging integration-test process from the user's config and logs.
- Keep all status and diagnostic paths read-only.

## Non-goals

- Lossy text decoding.
- Changing `file.read` behavior or the shared prefix-sniff contract.
- Changing the outer success semantics of `run.check`.
- Adding arbitrary command results, stdout, stderr, or child argv to logs.
- Extending the stable plugin request/response ABI.
- Disabling production logging when tests run.

## Design

### 1. Classified full-text reads for ctx commands

Add an internal ctx I/O helper whose result distinguishes valid text from invalid UTF-8:

```text
TextRead::Text(String)
TextRead::Binary
```

The helper performs a strict full read:

- valid UTF-8 returns `Text`;
- `io::ErrorKind::InvalidData` returns `Binary`;
- every other I/O failure remains an `AppError`.

Both `ctx.pack` and `ctx.symbols` use the helper after the existing safety inspection and text-candidate check. A `Binary` result is recorded with the existing `TextFileSkipReason::Binary` behavior and increments `skipped_binary_files`.

Existing output semantics remain unchanged:

- `ctx.pack` keeps the file item and file count, with zero lines and symbols;
- `ctx.symbols` omits the file from the symbol-bearing file list.

The prefix sniff remains unchanged. It intentionally cannot prove that the entire file is valid UTF-8 and may end on a partial multibyte sequence.

### 2. Partial managed-service status after scheduler failure

`status_snapshot` must not return immediately after `scheduler.inspect` fails. It records the scheduler failure first:

- `registration.status = scheduler_error`;
- scheduler state, diagnostic, and HRESULT remain populated;
- scheduler-derived semantic drift is not inferred.

The status path then attempts independent observation through a trusted chain:

```text
validated current pointer
  -> canonical managed definition
  -> durable runtime state
  -> exact-identity readiness probe
```

The current pointer and definition must pass the existing managed-path and identity checks. An arbitrary or stale definition path is never probed. Runtime reduction receives unverified scheduler-error evidence rather than a fabricated observed task.

The partial-status finalization path must preserve `scheduler_error`; later drift or installation reducers cannot overwrite it. If the trusted current/definition chain is absent or invalid, runtime and readiness remain at their deterministic default states, with readiness `not_checked`.

The entire status operation remains read-only. Tests compare durable state before and after observation.

### 3. Safe run.check completion outcome

Introduce an internal typed projection:

```text
RunCheckOutcome {
    success: bool,
    timed_out: bool,
    exit_code: Option<i32>,
}
```

Only this projection may enter logs. It excludes stdout, stderr, child argv, environment data, and arbitrary plugin response fields.

For CLI execution, the built-in `run.check` path returns the projection through an internal observation/completion envelope alongside the normal invocation result. The envelope is not serialized into plugin request/response JSON and does not extend the plugin ABI.

For synchronous MCP calls, the same projection is extracted from the typed structured result at the MCP boundary. For detached jobs, it is extracted before the typed result is moved into the job registry. Wrapper failures, cancellation, and infrastructure timeouts do not fabricate a child outcome.

The command-completion event and JSON log record gain an optional `outcome` field. It is present only for a completed `run.check` result:

```json
{
  "status": "success",
  "outcome": {
    "success": false,
    "timed_out": false,
    "exit_code": 7
  }
}
```

The outer `status` remains `success` when AIHelper successfully executes the `run.check` contract, even if the child process exits non-zero. A launch or wrapper failure remains an outer error and has no outcome. The additive optional log field does not require a schema-version increment.

### 4. Per-process integration-test config isolation

Add shared test command fixtures for `assert_cmd::Command` and long-lived `std::process::Command` use cases. Each fixture owns:

- the child command;
- a unique temporary directory;
- an `AH_CONFIG_DIR` environment override pointing to that directory.

The temporary-directory owner must live until the child exits. A helper must not return a bare command after dropping its temporary directory.

All integration-test launches use the shared isolated fixture except logging tests and tests that deliberately exercise config-directory behavior. Those tests continue to set explicit directories and remain responsible for their lifetimes.

A process-global `set_var`, a shared static directory, and a production `AH_LOG_DISABLED` switch are rejected because they introduce parallel-test races or reduce coverage of production startup behavior.

## Error Semantics

- Only strict UTF-8 decode failure becomes a binary skip; permission, disappearance, and read failures still fail the command.
- Valid UTF-8 split at the prefix-sniff boundary remains valid text.
- Scheduler failure remains visible even when runtime/readiness are independently healthy.
- Invalid trusted state never causes a readiness request to an unverified endpoint.
- A child non-zero exit is an observed outcome, not an AIHelper invocation failure.
- A timed-out child has `timed_out = true` and may have `exit_code = null`.
- Cancellation or wrapper failure has outer error status and no child outcome.
- Outcome state cannot leak into a subsequent command.

## Compatibility

- No released command response field is renamed or removed.
- No plugin request, response, or C ABI type is extended.
- Existing `run.check` stdout and process-exit behavior is unchanged.
- Existing event records remain valid; `outcome` is optional and command-specific.
- Log schema version remains `1` because the change is additive.

## Test Plan

### ctx

- Invalid byte after more than 8192 valid ASCII bytes: `ctx.symbols` succeeds, skips the file, and processes a valid neighbor.
- The same file in `ctx.pack` remains represented with zero lines/symbols and increments the binary skip count.
- Valid multibyte UTF-8 at the sniff boundary is not classified as binary.
- A real full-read I/O failure is not converted into a binary skip.

### Managed service status

- Scheduler inspection failure plus valid current, definition, runtime, and ready endpoint preserves scheduler/registration error while reporting ready runtime/readiness.
- Missing or invalid trusted definition leaves readiness `not_checked` and performs no untrusted probe.
- Durable current, definition, and runtime bytes are unchanged after status collection.

### Logging

- CLI, synchronous MCP, and MCP job paths cover child exit zero, non-zero exit, and timeout.
- Child non-zero exit preserves outer success and emits `outcome.success = false`.
- Cancellation or wrapper failure emits outer error without `outcome`.
- Serialized outcome never contains stdout, stderr, child argv, or secrets.
- Non-`run.check` commands do not emit `outcome`.
- Sequential commands do not inherit a previous outcome.

### Integration isolation

- Parallel fixtures receive different config directories.
- Long-lived child fixtures keep their directories alive through `wait`.
- Raw `cargo_bin("ah")` use is limited to the shared helper and intentional logging/config tests.
- Ordinary integration tests produce no records under the user's configured log directory.

## Risks and Mitigations

- **Scheduler error overwritten by later reducers:** use a distinct partial-observation finalization path and assert the final registration state.
- **Stale current pointer trusted:** reuse canonical managed-path and service/config/user identity validation before probing readiness.
- **Sensitive output leaked through generic serialization:** construct the outcome from an explicit three-field allowlist.
- **Outcome lost in detached jobs:** capture it before moving the typed response into job storage.
- **Temporary directory dropped before a child exits:** make directory ownership part of the command fixture type.
- **Concurrent user changes overwritten:** keep the implementation focused and preserve all pre-existing working-tree modifications as authoritative.

## Validation

Run focused tests while iterating, then the repository checks:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

