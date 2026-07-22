# MCP Active Job Lifecycle Integration Tests

## Scope

Complete the roadmap item for HTTP MCP lifecycle endpoint integration coverage
and active-job shutdown behavior. The change should validate the existing
contract without changing the public MCP API or production behavior unless the
new process-level test exposes a real defect.

## Current coverage

The integration suite already verifies the lifecycle endpoints independently:

- readiness works without an MCP session and returns the exact version, PID,
  and instance identity;
- malformed, stale, and non-local shutdown requests are rejected;
- a matching shutdown request returns `202 Accepted` and the process exits with
  code `0`;
- shutdown timeout and fatal startup failures exit with code `1`.

The missing scenario is a control shutdown while a real background job is
physically executing.

## Considered approaches

### Extend the existing control-shutdown test

This minimizes the number of tests, but combines request validation, identity
handling, process exit, admission closure, job cancellation, and child-process
cleanup in one large case. Failures would be harder to localize.

### Add a dedicated process-level lifecycle test

Start a real `ah.job.start` target through HTTP MCP, trigger control shutdown,
and inspect process and filesystem evidence. This exercises the complete stack
while leaving the existing endpoint validation test focused. This is the
selected approach.

### Use only an in-process fake executor

This would be fast and deterministic, but it could not prove that cancellation
reaches a real `ah.run.check` child process or that the top-level process exits
cleanly.

## Test design

Add one dedicated integration test to `tests/integration/mcp.rs`:

1. Start the HTTP MCP process with isolated configuration and workspace
   directories.
2. Read `/health/ready` and retain the exact instance identity.
3. Connect an MCP session and start an `ah.job.start` target for
   `ah.run.check`.
4. The child command writes a ready marker, waits for a release marker for much
   longer than the five-second shutdown budget, and writes a completion marker
   only if it survives long enough to observe that release marker.
5. Wait for the ready marker and query `ah.job.status` to prove that the job is
   physically running rather than merely admitted.
6. Send `/control/shutdown` with the ready instance identity and require the
   exact `202 Accepted` lifecycle response.
7. After `202`, attempt a new marker-writing command. A closed connection,
   stopped session, or deterministic executor-shutdown error is acceptable, but
   the marker must never be created.
8. Require the MCP process to exit within the existing bounded deadline with
   code `0`.
9. Create the release marker after process exit. If the child process survived
   shutdown, it would write the completion marker; assert that the completion
   marker remains absent.
10. Inspect the event log and require one cancelled `run.check` completion with
    the original `job_id`, plus a successful `mcp.serve` completion.

The blocking child command remains cross-platform: PowerShell on Windows and
`sh` on Unix. A release-marker guard ensures failed assertions do not leave the
child blocked indefinitely.

## Error handling and determinism

The post-acceptance admission probe must tolerate transport closure because the
server is allowed to stop before a new HTTP request reaches it. If a structured
MCP response is returned, it must not report successful execution. The durable
assertion is that the side-effect marker is absent.

Filesystem markers avoid timing-based assumptions about when a job became
active. The completion-marker handshake proves physical child termination
without waiting for the child command's long natural timeout.

Existing process guards remain responsible for terminating the MCP process if
the test panics. Temporary directories isolate all markers and event logs.

## Documentation and roadmap

No command-reference change is required because behavior is unchanged and the
existing MCP reference already documents admission closure, job cancellation,
the shared shutdown budget, and in-memory result loss.

After the focused and full validation suites pass, remove only this completed
roadmap item:

```text
Добавить integration tests lifecycle endpoints и завершения активных jobs.
```

Keep the target-client compatibility item unchanged.

## Validation

Run the focused new integration test first, followed by:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

Also inspect the scoped diff and verify that unrelated user changes remain
unstaged and unmodified.

## Acceptance criteria

- Readiness identity is used for the accepted control shutdown.
- An active background job receives cancellation during shutdown.
- New work cannot start after shutdown acceptance.
- The real child process does not survive server exit.
- The server exits cleanly inside the bounded lifecycle deadline.
- Event logs retain deterministic cancellation and serve-completion evidence.
- The completed roadmap item is removed only after all checks pass.
