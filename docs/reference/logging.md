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

Each completed command produces one `command.completed` JSON object containing:

- timestamp, PID, transport, command, and duration;
- sanitized CLI argv or typed MCP parameters;
- `success` or `error` status;
- a structured diagnostic only for errors;
- MCP tool and request IDs when applicable.

Startup, configuration, plugin discovery, MCP server, and transport problems use
separate `system` records. Successful result data, stdout, and stderr are not
stored.

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
