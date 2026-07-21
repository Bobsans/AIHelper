# Executor `max_active` Validation Design

## Scope

Prevent invalid concurrency limits from reaching Tokio's semaphore constructor
and causing a process panic.

## Design

`ParallelExecutor::new` validates that `max_active` is non-zero and does not
exceed `tokio::sync::Semaphore::MAX_PERMITS`. Invalid values return
`RuntimeError::InvalidExecutionRequest` through the existing error mapping.

Keeping the invariant in the executor protects CLI and non-CLI callers alike and
does not change valid concurrency behavior.

## Verification

Test the maximum supported value and the first value above it, asserting an error
instead of a panic.
