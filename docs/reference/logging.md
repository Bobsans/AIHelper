# Invocation Logging

AIHelper writes best-effort structured invocation logs for completed CLI
commands and MCP tool calls. Logging does not change command output, exit status,
or MCP JSON-RPC stdout.

## Location

Logs are stored below the global configuration directory:

- Windows: `%APPDATA%/AIHelper/logs`
- Linux: `$XDG_CONFIG_HOME/aihelper/logs` or `$HOME/.config/aihelper/logs`
- macOS: `$HOME/Library/Application Support/AIHelper/logs`

`AH_CONFIG_DIR` overrides the configuration root. Files use UTC dates:

```text
aihelper-YYYY-MM-DD.jsonl
```

The current date and nine previous UTC dates are retained. Future-dated files
are preserved to avoid data loss during midnight races or clock skew and age
into the normal window over time. Cleanup and writes are best effort; logging
failures never fail a command.

## Records

Each completed command normally produces one `command.completed` JSON object, except
standalone `ah --version` and `ah -V` fast-path requests. Records contain:

- timestamp, PID, transport, command, and duration;
- sanitized CLI argv or typed MCP parameters;
- `success` or `error` status;
- a structured diagnostic only for errors;
- a safe `outcome` for completed `run.check` calls, containing only child
  `success`, `timed_out`, and `exit_code`;
- MCP tool and request IDs when applicable, plus `job_id` for detached targets.

MCP execution records retain `queue_wait_ms` for schema compatibility, but the
parallel executor never queues work and reports it as zero. `execution_ms`
measures admission to logical completion. `duration_ms` remains total MCP adapter
time and can be slightly greater. Timed-out calls use
`timeout_phase: "execution"`. Calls rejected before admission omit these optional
fields.

Startup, configuration, plugin discovery, MCP server, and transport problems use
separate `system` records. Other successful result data, stdout, stderr, and
child argv are not stored. A non-zero child exit keeps the outer command status
at `success` and records `outcome.success: false`.

One `system` record has no `command.completed` beside it, because the command
never ran. When an interrupted update is recovered before startup, the invocation
is consumed by that recovery and writes a record with component
`update-recovery`, severity `warning` and diagnostic code
`UPDATE_RECOVERY_CONSUMED_INVOCATION`, whose context carries the redacted argv
and the transaction it acted on. It is written whatever the output mode, so an
invocation that did something other than what was asked is auditable after the
fact. See [upgrade](upgrade.md).

## Redaction

AIHelper redacts recognized passwords, tokens, authorization values, cookies,
credentials, private keys, curl user credentials, sensitive JSON fields, and
related CLI/header/URL forms. Strings, collections, nesting, and complete
records are bounded. Arbitrary secrets in opaque positional shell arguments
cannot always be recognized.

Set the following only for isolated diagnostics when full parameter values are
required:

```text
AH_LOG_UNREDACTED=1
```

This disables secret substitution but keeps size limits. Unredacted logs may
contain credentials, source code, HTTP bodies, SQL, and shell arguments.

## Delivery

Concurrent processes use a short cross-process lock before appending a complete
JSONL record. Lock contention is bounded to 50 ms. Filesystem operations can
still experience normal filesystem latency. Disk, permission, serialization,
or lock failures may drop an event under the best-effort contract.

MCP submits completed-call events to a process-local queue of 256 entries. One
dedicated logging thread drains that queue, so event-sink latency never occupies
Tokio command workers or delays an MCP response. A full or disconnected queue
drops the new event instead of blocking execution. Sink panics are contained and
do not terminate the MCP request or dispatcher. During normal MCP shutdown, a
flush barrier writes preceding healthy events within the remaining shared
shutdown budget; a stuck sink cannot extend that budget.
