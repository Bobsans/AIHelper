# MCP Job Namespace Reservation Design

## Scope

Prevent plugin commands from being published under the system-owned `ah.job.*`
namespace and then intercepted by built-in job routing.

## Design

MCP catalog construction rejects any plugin descriptor whose MCP tool name starts
with `ah.job.`. The adapter returns a clear invalid-schema error before either
stdio or HTTP serving begins.

The four built-in job tools remain the only occupants of the namespace. Their
construction is internal and occurs after plugin descriptor validation.

## Verification

Add a plugin fixture that publishes `job.extra` and assert that `McpServer::new`
rejects it. Document the reserved namespace in the MCP command reference.
