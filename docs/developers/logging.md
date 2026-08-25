# Command Event Logging

AIHelper records sanitized command-completion and system events without changing
command output, exit status, or MCP protocol behavior. See the
[invocation logging reference](../reference/logging.md) for user-facing paths,
fields, configuration, and delivery guarantees.

## Instrumentation Boundaries

Instrumentation belongs at the root CLI runtime and the complete MCP `call_tool`
boundary. A routed, caller-visible invocation produces one `command.completed`
event regardless of whether a built-in or dynamic plugin handles it. Failures
before a command is routed, plus startup, configuration, plugin discovery, MCP
server, and transport failures, use separate `system` events.

Do not add event logging inside individual commands, plugins, `PluginManager`, or
the executor. Boundary instrumentation keeps completion semantics centralized and
does not extend the plugin ABI.

## Failure Isolation and Delivery

Logging is best effort. Initialization, serialization, locking, retention, sink,
and write failures may drop an event but must never change the caller-visible
result. The logger writes only to its JSONL files and never to CLI streams or MCP
stdout.

Producers sanitize and bound an event before delivery. Long-lived MCP servers use
a bounded process-local dispatcher backed by a dedicated thread. Command handlers
do not wait for queue capacity; a full or disconnected queue drops the new event,
and slow, failed, or panicking sinks remain isolated from command execution.

## Records and Status

Records use `schema_version: 1`. Command records use `command.completed`; startup
and infrastructure records use `system`. Optional fields are omitted instead of
serialized as `null`. Successful result data, stdout, and stderr are not stored,
except for the typed `run.check` outcome projection containing only `success`,
`timed_out`, and nullable `exit_code`.

Status describes the outer AIHelper invocation. A command or adapter error is an
error completion. Data returned by a successful command does not redefine that
status: for example, a successful `run.check` response containing
`success: false` is still logged as a successful AIHelper invocation. The child
result is represented by `outcome.success: false`; wrapper errors and cancellation
have no outcome.

## Redaction and Bounds

Redaction lives in `crates/ah-redact` and is shared by every sink; do not add a
local redaction helper to a sink.

Redaction happens before an event reaches the dispatcher or writer. Sanitize
sensitive JSON keys, CLI flags, headers, URL credentials and query values, plus
diagnostic messages and causes. Apply these limits after redaction:

- 4 KiB per string, truncated on a UTF-8 boundary;
- 100 entries per array or object;
- nesting depth of 8;
- 64 KiB per complete JSONL record, including its newline.

Oversized records are compacted and, if necessary, replaced by a valid minimal
record that preserves required envelope fields and reports truncation.
`AH_LOG_UNREDACTED=1` disables secret substitution only; all size limits remain
active. This mode can expose credentials, source code, request bodies, SQL, and
shell arguments.

## Storage and Retention

Write daily UTC files below the global configuration directory and retain the
current date plus nine preceding dates. Cleanup deletes only regular,
non-symlink files with an exact AIHelper log filename older than that window.
Future-dated, unrelated, and malformed filenames remain untouched.

Serialize and bound the complete line before append. Concurrent processes use a
short cross-process lock with at most 50 ms of lock acquisition retries, then
append one complete line. All filesystem and cleanup errors remain under the
best-effort contract.

## Verification

Cover sanitization, UTF-8 truncation, record compaction, injected-clock rollover,
and retention with unit tests. CLI and MCP integration tests must prove one
completion per accepted caller-visible invocation, correct system-event
boundaries, stable status semantics, and no protocol-stream contamination.

Concurrency and failure tests must parse every appended JSONL line, detect
interleaving, exercise queue saturation and sink panics, and compare command
output and status with logging healthy, unavailable, lock-contended, and failing.
