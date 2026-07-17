# MCP Runtime Hardening Design

**Date:** 2026-07-17  
**Status:** Approved for implementation

## Context

The first stdio MCP implementation exposes typed tools for every AIHelper
domain, but review found lifecycle races, avoidable catalog work, unsafe
persistence edges, and two search defects. The fixes must preserve the released
CLI JSON fields, the flat MCP tool input shape, stdio-only transport, and the
current plugin ABI. They must also leave a clean boundary for a future bounded
parallel executor.

## Goals

- Make queued, active, cancelled, timed-out, and draining execution states
  deterministic.
- Keep destructive handlers strictly sequential after a timeout until the
  timed-out handler actually exits.
- Make queued cancellation and total queue deadlines observable without waiting
  for the active handler.
- Compile typed command catalogs and JSON Schema validators once per definition
  revision.
- Keep MCP tool snapshots consistent with live plugin enabled-state changes.
- Prevent host control-plane domains from being shadowed by dynamic plugins.
- Make plugin settings and task-store updates transactional and crash-safe.
- Bound search result allocation by the requested limit and return exact context.

## Non-goals

- HTTP MCP transport.
- Parallel command execution in this release.
- Forced termination of arbitrary in-process plugin code.
- A new typed plugin ABI symbol or capability.
- Fully streaming text search.
- Live reload of plugin binaries or external settings changes.

## Chosen Approach

Use a focused infrastructure refactor rather than independent local patches.
The runtime owns execution lifecycle and the typed registry; domains only own
their command work and cooperative cancellation scopes. Persistence uses one
shared transactional helper. This is larger than a hotfix but avoids rebuilding
the same boundaries when bounded parallel execution is introduced.

## Execution Coordinator

`SequentialExecutor` keeps one worker, backed by a coordinator that owns
admission and request state under one mutex. Request transitions are:

```text
Queued -> Running -> Finished
   |         |          ^
   |         +-> CancelRequested
   |         +-> TimedOutDraining
   +-> Cancelled
   +-> TimedOut
```

The queued-to-running transition and cancellation check happen atomically under
the coordinator lock. Plugin cancellation is invoked only after releasing that
lock.

Each call receives a monotonically unique internal execution ID. The MCP
adapter maintains the short-lived mapping from protocol request ID to execution
ID used by cancellation notifications. Protocol IDs may therefore be reused
after a response without colliding with late worker cleanup.

Cancellation and deadline signals wake the waiting `execute` future directly:

- queued cancellation returns immediately and leaves a cancelled queue
  tombstone for the worker to discard;
- a queued deadline returns `TIMEOUT` immediately even if another handler is
  still running;
- active cancellation invokes the plugin cancellation path and suppresses late
  success;
- an active deadline enters `TimedOutDraining`, returns `TIMEOUT`, and closes
  admission for new execution requests;
- while draining, new calls fail with retryable `EXECUTOR_DRAINING` and include
  the blocking execution ID;
- requests accepted before draining keep their original FIFO order and
  deadlines, but none starts before the draining handler exits;
- ordinary completion, panic, and worker failure observed before the deadline
  remove tracked state before notifying the caller;
- admission reopens only after the timed-out handler has actually exited.

The stdio control loop, `tools/list`, pings, and cancellation notifications stay
responsive while execution admission is closed.

The coordinator uses a set of draining execution IDs even though v1 can contain
only one. A future bounded parallel executor can replace the worker with lanes
or a semaphore without changing request identity or lifecycle semantics.

## Cooperative Cancellation Scopes

The `run`, `task`, and `search` built-ins and the GitHub/GitLab typed adapters
use request-local RAII scopes:

1. install the unique execution ID in thread-local context;
2. preserve an already-delivered cancellation marker;
3. check cancellation before beginning command work;
4. remove thread-local and registry state in `Drop`, including panic paths.

The handler must never clear a cancellation marker on entry. Dynamic ABI
cancellation remains the existing `ah_plugin_cancel_command_v1` symbol. Unique
execution IDs and adapter-side active mappings prevent late notifications from
targeting a later request.

## Typed Registry and MCP Catalog Snapshots

`PluginManager` owns a lazily built immutable typed registry containing:

- commands indexed by command ID;
- a deterministically sorted command list;
- plugin owner, source, and whether the command is disableable;
- compiled input and output JSON Schema validators.

Registration or dynamic discovery invalidates the definition revision. Changing
the disabled-domain set increments an enabled-state revision only when the set
actually changes. Normal invocation looks up one registry entry and uses its
compiled validators; it does not rebuild a domain catalog.

`McpServer` caches an immutable tool snapshot indexed by MCP name. It rebuilds
the snapshot only when the manager catalog revision changes. Plugin mutations
compare revisions to decide whether to send `notifications/tools/list_changed`;
full-catalog serialization and fingerprinting are removed from the call path.

Snapshot locks are held only while cloning an `Arc`, so catalog reads do not
serialize future parallel command execution.

## MCP-compatible Input Schema

The flat tool shape remains:

```json
{
  "command_field": "value",
  "context": {
    "cwd": ".",
    "limit": 20,
    "timeout_ms": 30000
  }
}
```

To make injection of the reserved `context` property deterministic, typed input
schemas must use a canonical root-object subset. Allowed root keywords are
`$schema`, `$id`, `$defs`, `definitions`, `title`, `description`, `default`,
`examples`, `deprecated`, `readOnly`, `writeOnly`, `type: object`, `properties`,
`required`, and `additionalProperties`. Nested property and local-definition
schemas remain unrestricted.

Root constraints that can contradict injected context are rejected, including
composition and conditional keywords, object-count constraints,
`propertyNames`, `patternProperties`, dependency keywords, root `enum`/`const`,
and `unevaluatedProperties`. The augmented MCP schema is compiled during
registry construction. An incompatible plugin catalog fails deterministically
before the server begins serving tools.

## Reserved Host Domains

The application reserves `ai`, `plugins`, and `mcp` for host control-plane
commands before dynamic plugin discovery. The runtime loader receives the
application-owned reserved set rather than hard-coding these names. A dynamic
plugin using a reserved domain is skipped with a deterministic load diagnostic
and never appears in plugin lists, typed catalogs, or invocation routing.

## Transactional JSON Persistence

Plugin settings and task stores share a persistence helper that performs:

```text
bounded sidecar lock
-> reload current file
-> clone and mutate
-> serialize
-> write a temporary sibling
-> flush and sync
-> atomic replace
-> publish in-memory state
-> unlock
```

The sidecar lock covers the complete read-modify-write transaction and works
across processes. Lock acquisition is bounded; failure returns a retryable
diagnostic rather than blocking the executor indefinitely. Temporary files are
created in the destination directory so replacement stays on one filesystem.

`PluginSettings` publishes the candidate to the shared in-memory object and
calls `PluginManager::set_disabled_domains` only after persistence succeeds.
CLI and MCP mutations use the same transactional API. A failed write therefore
changes neither live manager state nor the retained settings object.

Task stores use a per-path in-process lock in addition to the sidecar lock, so
independent workspaces can proceed independently under a future parallel
executor. Readers observe either the old complete JSON document or the new
complete document; concurrent writers do not lose unrelated task updates.

## Search Corrections

Text search keeps the current bounded full-file safety policy but changes result
collection:

- context-before starts at `index.saturating_sub(context_lines)`;
- the file text is held once and indexed as `Vec<&str>` instead of cloning every
  line;
- the remaining global result budget is applied inside the per-file scan;
- one sentinel match beyond the budget determines exact `truncated` state;
- cancellation is checked during line scanning, not only between files;
- context strings are allocated only for matches that enter the bounded result.

Fully streaming context assembly is deferred because overlapping matches and
after-context require a more complex pending-match state machine. The existing
`max_bytes` policy continues to bound each file read.

## Error Model

New execution admission failures use stable diagnostic code
`EXECUTOR_DRAINING`, are retryable, and identify the blocking execution. Existing
`TIMEOUT` and `EXECUTION_CANCELLED` behavior remains. Catalog, schema, lock, and
atomic-replace failures retain structured domain and operation context and never
emit partial success data.

## Testing

Deterministic tests use barriers or condition variables rather than timing-only
assertions.

Executor coverage includes:

- cancellation at every queued-to-running boundary;
- cancellation delivered before handler scope installation;
- queued cancellation and queued timeout while another handler is blocked;
- uncooperative timeout, immediate `EXECUTOR_DRAINING`, and admission recovery;
- no overlap between draining and queued handlers;
- protocol request ID reuse and late cleanup;
- handler panic cleanup.

Registry and MCP coverage includes:

- catalog and validators built once per definition revision;
- enabled-state revision changes only for real mutations;
- concurrent snapshot reads see complete old or new catalogs;
- reserved dynamic domains are skipped;
- incompatible root schemas are rejected;
- compatible closed-object schemas receive the optional context property.

Persistence coverage includes:

- save failure leaves memory and manager unchanged;
- successful reload equals live state;
- concurrent writers preserve unrelated updates;
- readers never observe partial JSON;
- lock timeout and temporary-file cleanup.

Search coverage includes exact context arrays for first, middle, and last-line
matches, sentinel truncation, early cancellation, duplicate roots, and a large
matching file whose returned allocation remains bounded by the limit.

## Acceptance Criteria

- All ten review findings have regression tests.
- A timed-out uncooperative handler never overlaps another handler.
- New calls are rejected while that handler drains and succeed after it exits.
- Queue deadlines expire independently of worker progress.
- Normal tool calls do not rebuild catalogs or compile schemas.
- Plugin state changes produce one consistent catalog revision and notification.
- Host domains cannot be shadowed.
- Settings and task JSON remain parseable and do not lose concurrent updates.
- Search returns exact requested context and bounded results.
- Existing CLI JSON fields, typed command IDs, stdio transport, and plugin ABI
  remain compatible.
- Formatting, workspace tests, debug build, and release build pass through `ah`.
