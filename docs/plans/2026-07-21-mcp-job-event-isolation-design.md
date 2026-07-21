# MCP Job Event Isolation Design

## Scope

Ensure a slow or panicking MCP `EventSink` cannot keep a background job running,
delay physical completion bookkeeping, or retain executor capacity.

## Design

The job runner commits the logical terminal result before invoking any observer.
It then schedules event delivery as detached blocking work and independently waits
for the execution lifecycle's physical completion before updating retention state.

The detached event callback is wrapped in `catch_unwind(AssertUnwindSafe(...))`.
Observer panic is contained inside the task, and observer latency does not delay
job state transitions. Event delivery remains best effort and does not change the
published command result.

## Verification

Add regression tests with panicking and deliberately blocked sinks. In both cases,
the job must expose its terminal result and complete physical bookkeeping without
waiting for or propagating the observer failure.
