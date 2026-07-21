# HTTP MCP readiness and instance identity

## Goal

Complete the first coherent slice of the managed MCP roadmap by exposing an
HTTP readiness endpoint that identifies the exact running AIHelper process.
The endpoint must work without creating an MCP session and must preserve the
local-only request policy used by the MCP endpoint.

## Scope

- Add `GET /health/ready` to the local HTTP transport.
- Return `status`, AIHelper binary `version`, process `pid`, and a unique
  process-lifetime `instance_id`.
- Keep one instance identity for every HTTP session served by the process and
  generate a new identity after a restart.
- Apply the same expected `Host` and optional local `Origin` policy to the
  readiness and MCP routes.
- Document the endpoint and add integration coverage.

Control shutdown, draining readiness, service management, and durable runtime
state remain outside this step.

## Contract

A ready server returns HTTP `200` with `application/json`:

```json
{
  "status": "ready",
  "version": "1.1.0",
  "pid": 1234,
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

The response contains no timestamp or request-specific data. Repeated requests
to the same process return the same values. A later process returns a different
UUID v4 `instance_id`, even if the operating system reuses a PID.

The version originates in the root AIHelper binary and is passed into the HTTP
transport. This avoids reporting the package version of the `ah-mcp` library
when it differs from the executable release version.

## Design

Create immutable process-wide HTTP instance metadata when the HTTP transport
starts. Share it with the Axum readiness handler independently of MCP sessions.
The shape should be suitable for extension into a lifecycle state during the
next control-shutdown step, without implementing shutdown admission changes in
this commit.

Use a typed serializable readiness response so field names and serialization
remain explicit. Generate the identity from operating-system randomness through
UUID v4 rather than combining PID and wall-clock time.

Move the local request policy to a reusable router-level check, or add an
equivalent shared check, so `/health/ready`, `/mcp`, and the future control route
accept the expected loopback authority, allow requests without `Origin`, accept
only the expected local origin when it is present, and emit no CORS headers.
The existing RMCP validation may remain as defense in depth for `/mcp`.

## Error handling

- Requests with an unexpected `Host` are rejected before the readiness handler.
- Requests with a present, unexpected `Origin` are rejected before the handler.
- Identity creation does not introduce a recoverable runtime branch: UUID v4
  generation uses the platform random source through the UUID library.
- Listener binding and other startup failures retain the existing MCP adapter
  error and non-readiness behavior.

## Verification

Integration tests will verify that:

- readiness succeeds without an MCP session;
- the response has the documented content type and exact field set;
- version matches the running `ah` binary and PID matches the child process;
- repeated requests to one process return the same identity;
- separate process launches return different identities;
- invalid `Host` and external `Origin` values are rejected;
- no CORS headers are emitted;
- existing HTTP MCP sessions and port-conflict behavior continue to work.

After the focused checks pass, run the applicable workspace formatting, tests,
and build checks. Then remove the completed readiness and version/PID/identity
items from the roadmap and commit the implementation as one roadmap step.
