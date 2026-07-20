# Windows Job Spawn Design

## Goal

Reduce Windows child-process startup latency while preserving the guarantee that
timeouts and cancellation terminate the complete process tree.

## Platform Contract

- The optimized backend requires Windows 10 or Windows Server 2016 and newer.
- Unix process-group behavior remains unchanged.
- Windows native executables are assigned to a Job Object atomically during
  `CreateProcessW` through `PROC_THREAD_ATTRIBUTE_JOB_LIST`.
- Batch scripts are invoked through the system `cmd.exe` using the same escaping
  rules as Rust's standard process implementation and remain inside the Job.

## Execution

The Windows backend creates restricted stdin/stdout/stderr pipes, a Job Object
with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and a process attribute list containing
the Job and inherited stdio handles. It then starts the process without a
suspend/resume phase. Timeout and cancellation use `TerminateJobObject`.

The parent keeps the root process exit status while waiting for the Job's active
process count to reach zero. This preserves process-tree lifetime semantics even
when the root exits before a descendant.

## Validation

- Preserve exit-code, cwd, argument, output capture, truncation, timeout, and
  cancellation contracts.
- Add Windows coverage for native argument quoting and process-tree cleanup.
- Run the complete workspace test suite on both Ubuntu and Windows CI.
- Target a warm-run median below 60 ms for `run check cmd /c exit 0` on the
  development machine.
