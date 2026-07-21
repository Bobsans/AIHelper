# MCP HTTP `SIGTERM` Shutdown Design

## Scope

Allow the local HTTP MCP daemon to shut down gracefully under Unix service and
process-manager termination, while preserving Windows console behavior.

## Design

Introduce one async shutdown-signal helper. On Unix it registers a terminate
signal stream and waits concurrently for `SIGTERM` or `Ctrl-C`. On non-Unix
platforms it waits for `Ctrl-C` as before.

After either signal, HTTP graceful shutdown closes executor admission and cancels
the shared HTTP session token. Existing bounded runtime shutdown handles physical
blocking workers.

## Verification

Add a Unix-only integration test that starts the local HTTP MCP server, sends
`SIGTERM`, and asserts that the process exits within the shutdown deadline.
