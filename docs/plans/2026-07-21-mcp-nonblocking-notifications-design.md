# MCP Non-blocking Notifications Design

## Scope

Prevent `notifications/tools/list_changed` delivery from delaying the command
that changed the MCP catalog.

## Design

Catalog refresh remains synchronous so the command response observes the new
tool list. Notification delivery becomes best-effort background work. The server
copies the registered peers, starts one Tokio task per peer, and returns without
awaiting any peer.

Each task applies a one-second timeout to its peer notification. A peer is removed
from the shared registry when delivery fails or times out. Successful peers remain
registered. Tasks do not hold the peer registry mutex while awaiting I/O.

This keeps clients isolated: a stalled or disconnected client cannot add latency
to commands from another client, and notifications to healthy clients run in
parallel.

## Verification

Add focused coverage for immediate caller return, timeout/error cleanup, and
successful peer retention where the RMCP peer test harness permits it. Run the
`ah-mcp` package tests after the change.
