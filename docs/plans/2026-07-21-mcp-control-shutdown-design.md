# HTTP MCP control shutdown

## Goal

Add deterministic programmatic shutdown for the local HTTP MCP process and make
Ctrl-C, SIGTERM, and the control endpoint use one bounded lifecycle path.

## Scope

- Add `POST /control/shutdown` to the local HTTP transport.
- Require the process-lifetime `instance_id` returned by readiness.
- Introduce one process-wide, idempotent HTTP lifecycle controller.
- Route Ctrl-C, SIGTERM, and accepted control requests through that controller.
- Preserve one shared five-second shutdown budget across transport draining,
  event flushing, physical handlers, and Tokio runtime shutdown.
- Add success, rejection, race, and process-exit coverage.
- Document the endpoint and remove the two completed roadmap items after all
  checks pass.

Readiness changes during draining, managed service commands, fatal exit-code
hardening, and the broader active-job lifecycle integration matrix remain
outside this step.

## HTTP contract

The request must use JSON and contain exactly one field:

```http
POST /control/shutdown
Content-Type: application/json

{"instance_id":"550e8400-e29b-41d4-a716-446655440000"}
```

The identity must be a UUID and must exactly match the running process. A valid
request returns HTTP `202 Accepted`:

```json
{
  "status": "shutting_down",
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

The response is sent before the process exits. A concurrent accepted request
that already reached the handler returns the same status and payload. Requests
made after Axum stops accepting connections may fail to connect.

## Error contract

Lifecycle request errors use deterministic JSON:

```json
{
  "error": {
    "code": "INSTANCE_ID_MISMATCH",
    "message": "instance_id does not match the running AIHelper instance"
  }
}
```

- `400 Bad Request` with `INVALID_SHUTDOWN_REQUEST` for malformed JSON, missing
  or unknown fields, non-string identities, and invalid UUIDs.
- `409 Conflict` with `INSTANCE_ID_MISMATCH` for a valid stale or foreign UUID.
- `415 Unsupported Media Type` with `UNSUPPORTED_MEDIA_TYPE` when the request is
  not JSON.
- `403 Forbidden` for an unexpected `Host` or external `Origin` without any
  lifecycle transition.
- `413 Payload Too Large` remains enforced by the existing global body limit.

Mismatch responses do not reveal the current identity. Clients must read the
readiness endpoint again before retrying. Instance identity is a stale-target
guard, not authentication.

## Lifecycle design

Extend the process-wide HTTP lifecycle state with an `Arc`-shared controller.
It owns an atomic `Ready -> ShuttingDown` transition, the existing shutdown
tracker, the shared executor, and the RMCP cancellation token.

The first `begin_shutdown()` call performs this sequence:

1. Atomically enter `ShuttingDown` and start the existing shutdown tracker.
2. Call `Executor::close()`. The parallel executor already closes admission and
   logically cancels every tracked direct execution and background job.
3. Cancel the RMCP HTTP token so sessions stop and new MCP requests are rejected.
4. Wake Axum graceful shutdown so it stops accepting connections and waits for
   in-flight HTTP handlers.
5. Flush events and shut down physical handlers and the Tokio runtime using only
   the remaining tracker budget.

Later `begin_shutdown()` calls are no-ops and do not restart or extend the
timeout. The control handler checks identity before attempting the atomic
transition. A mismatching request therefore never changes admission or session
state, including when it races with an unrelated signal.

The graceful-shutdown future waits for either an operating-system signal or the
controller token. A signal calls the same `begin_shutdown()` method as the HTTP
handler. Existing fatal bind behavior remains an error and is not converted into
a successful intentional shutdown.

## Local request policy

The control endpoint uses the same policy as readiness and MCP:

- the listener remains bound to `127.0.0.1`;
- `Host` must exactly match `127.0.0.1:<port>`;
- `Origin` may be absent, but a present value must exactly match the local
  origin;
- CORS, TLS, and authentication remain disabled.

Identity validation and shutdown side effects happen only after this policy
accepts the request.

## Verification

Unit or in-process tests will verify that concurrent lifecycle triggers execute
the transition once and never extend the timeout.

Integration tests will verify that:

- a matching identity returns the exact `202` JSON before clean process exit;
- an old or foreign identity returns `409` and leaves readiness operational;
- malformed schemas and media types return the documented errors without
  stopping the process;
- hostile Host and Origin values return `403` and do not stop the process;
- accepted responses contain no CORS headers;
- control shutdown works without creating an MCP session;
- existing SIGTERM behavior still exits successfully through the shared path;
- port-conflict startup behavior remains unchanged.

After focused tests, run formatting, locked workspace tests, and the locked debug
build. Review the complete scoped diff before updating the roadmap and creating
the implementation commit.
