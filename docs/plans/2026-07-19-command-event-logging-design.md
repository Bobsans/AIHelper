# Command Event Logging Design

## Status

Approved for implementation on 2026-07-19.

## Context

AIHelper currently returns structured errors to CLI and MCP callers, but it does
not retain invocation context after a process exits. Diagnosing intermittent
failures therefore depends on reproducing the command and preserving external
client output.

Logging must cover CLI commands, MCP tool calls, dynamic plugins, and failures
outside a command such as configuration, plugin discovery, and MCP transport.
It must not change command behavior, expose secrets by default, or write
non-protocol data to MCP stdout.

## Goals

- Write one completion record for every routed CLI command, CLI help/version
  request, and accepted MCP tool call.
- Include sanitized parameters, status, duration, and process/request identity.
- Include a structured diagnostic only when the AIHelper invocation fails.
- Record startup, configuration, plugin discovery, and MCP transport problems as
  separate system events.
- Store daily JSONL files in the global AIHelper configuration directory.
- Retain logs for the current UTC date and the preceding nine UTC dates.
- Support multiple concurrent CLI and MCP processes without interleaved records.
- Keep logging best effort: logger failures never change output, exit status, or
  MCP protocol behavior.

## Non-Goals

- Successful response bodies, stdout, stderr, or typed result data are not
  retained.
- The logger is not a general tracing framework or metrics system.
- Exactly-once persistence is not guaranteed during disk failure, process
  termination, or prolonged lock contention.
- The plugin ABI is not extended for logging.
- Arbitrary secrets embedded in opaque positional arguments cannot be detected
  perfectly.

## Chosen Architecture

Use explicit instrumentation at the CLI and MCP adapter boundaries.

The root application owns a best-effort `EventLogger`. CLI execution records
events directly. The MCP adapter accepts an optional `Arc<dyn EventSink>` and
emits normalized events to the same logger. Existing `McpServer::new` behavior
remains available with no sink so library consumers are not forced to configure
logging.

Do not log inside individual commands, plugins, `PluginManager`, or the typed
executor. Boundary instrumentation provides one event per caller-visible
invocation and automatically includes built-in and dynamic plugins without ABI
changes. It also covers errors that occur before plugin dispatch, including MCP
tool lookup, context validation, queue admission, timeout, and cancellation.

General-purpose `tracing` is not used for the command event contract. Redaction,
daily retention, cross-process append safety, and exact completion semantics
would still require a custom layer, while asynchronous writers can lose the
final event of short-lived CLI processes.

## Event Boundaries

### CLI

Initialize the logger best effort at the start of `runtime_flow::run`, before
the main configuration is loaded. Derive the log directory from non-empty
`AH_CONFIG_DIR` or the existing platform-specific global configuration path.
Failure to resolve or create this directory disables logging for that event.

Create a lightweight invocation envelope from raw argv before configuration and
routing. After routing produces a `RuntimeCommand`, attach the canonical command
identity and sanitized parameters. Emit exactly one `command.completed` record
after `execution` returns.

Failures before a valid command can be routed are `system` events only; they do
not create a synthetic `command.completed` record. Components include:

- `startup`
- `config`
- `cli_parse`
- `plugin_discovery`

Help and version are successful CLI invocations with canonical command values
`help` and `version`. Abrupt process termination may produce no completion
record. The routed `mcp.serve` command always receives one CLI completion record
when the server returns. Normal stdio EOF is a successful completion and does
not create a system event. Server construction or transport failure makes that
CLI completion an error and additionally emits one corresponding system event.
Individual MCP calls receive their own records.

### MCP

Instrument the complete `call_tool` boundary around `call_tool_inner`. Use the
requested tool name for unknown tools and the canonical descriptor ID for known
commands. Include the execution request ID when available.

The completion event covers:

- successful typed responses;
- typed command errors;
- unknown tools;
- invalid request context;
- queue admission errors;
- timeout and cancellation;
- plugin and response-validation errors.

MCP server construction and stdio transport failures are separate `system`
events and never synthesize an MCP tool completion. They can also determine the
status of the already-routed `mcp.serve` CLI command as described above. If a
tool call completes and the transport subsequently fails while sending its
response, retain the tool's `command.completed` record and add a separate
`mcp_transport` system event. The logger writes files directly and never uses
stdout or stderr.

## Status Semantics

`status` describes the outer AIHelper invocation envelope.

- `InvocationResponse.success=true` is `success`.
- `TypedInvocationResponse.success=true` is `success`.
- `AppError` or a typed response with `success=false` is `error`.
- Tool-call adapter, queue, timeout, cancellation, and response-validation
  failures are `error` command completions.
- MCP server and transport failures outside an accepted tool call do not create
  an MCP tool completion. They emit system events and may also fail the routed
  `mcp.serve` CLI completion.
- A successful `run.check` response whose data contains `success=false` remains
  `success`; the external process result is data, not an AIHelper failure.

Successful records omit response/result fields. Error records contain a
sanitized structured diagnostic.

## Command Record Schema

The schema is versioned from the first release.

```json
{
  "schema_version": 1,
  "timestamp": "2026-07-19T14:25:31.123Z",
  "event": "command.completed",
  "transport": "mcp",
  "pid": 1234,
  "request_id": "mcp:n:7:e:1",
  "command": "plugins.list",
  "tool": "ah.plugins.list",
  "parameters": {},
  "status": "success",
  "duration_ms": 12
}
```

Required command fields are `schema_version`, `timestamp`, `event`, `transport`,
`pid`, `command`, `parameters`, `status`, and `duration_ms`. `event` is always
`command.completed`; `transport` is `cli` or `mcp`; `status` is `success` or
`error`. Optional fields are omitted rather than serialized as null.

`request_id` and `tool` are MCP fields. Known MCP tools use the descriptor ID as
`command` and the exposed MCP name as `tool`. Unknown tools use the sanitized
requested name for both fields. CLI host commands use their canonical IDs, for
example `plugins.list`.

All CLI command records use `{"argv":[...]}` as `parameters`, containing the
sanitized raw process arguments in their original order but excluding the
executable path. This applies uniformly to host commands, plugin commands,
`mcp.serve`, help, and version. Pre-routing system events place the same argv
array in `context.argv`.

For command identity, a CLI `RuntimeCommand::Invoke` additionally uses its
normalized plugin argv after host-global flags have been removed. Resolve the
command by selecting the longest enabled catalog descriptor whose ID segments
after the domain equal the leading normalized argv items. For example,
`git tag create ...` resolves to `git.tag.create`. If the plugin has no typed
catalog or no descriptor matches, use `<domain>.<first-argv-item>` when present,
otherwise the domain. This fallback is an operation hint, not a claim that
plugin parsing succeeded.

Help/version use `help` and `version`. Dynamic plugin calls follow the same
catalog and fallback rules without plugin-specific handling.

`parameters` is always a JSON object. CLI opaque argv is stored under `argv`;
typed MCP arguments retain their object shape. `diagnostic` is permitted only
when `status` is `error`. `record_truncated` is an optional boolean indicating
whole-record compaction.

An error record adds a diagnostic. Normal diagnostics require `code`, `message`,
and `exit_code_hint`; `domain`, `operation`, `cause`, and `retryable` are
optional when the source does not provide them.

```json
{
  "diagnostic": {
    "domain": "search",
    "operation": "search.text",
    "code": "REGEX_INVALID",
    "message": "invalid regular expression",
    "cause": "unclosed group",
    "exit_code_hint": 1,
    "retryable": false
  }
}
```

CLI diagnostics come from `AppError::diagnostic`. MCP diagnostics come from
`CommandError` or an adapter-generated equivalent.

## System Record Schema

System records require:

- `schema_version`
- `timestamp`
- `event: "system"`
- `pid`
- `component`
- `severity` (`warning` or `error`)
- a sanitized structured `diagnostic`
- a bounded `context` object, which may be empty

Valid initial components are `startup`, `config`, `cli_parse`,
`plugin_discovery`, `mcp_server`, and `mcp_transport`. Warnings that do not
already have an `AppError` or `CommandError` use a synthetic stable diagnostic
code rather than an unstructured top-level message. Every system diagnostic,
including warnings, requires `code`, `message`, and `exit_code_hint`. Synthetic
warning diagnostics use `exit_code_hint: 0`; error diagnostics use the available
application hint or `1`. `domain`, `operation`, `cause`, and `retryable` remain
optional. The minimal system fallback follows the same required diagnostic
shape, using `message: "diagnostic truncated"` when compaction is necessary.

Plugin discovery warnings are recorded independently of `--quiet`. A later
command completion remains a separate event.

## Redaction And Bounds

Redaction happens before data reaches the writer or a shared queue.

Tokenize parameter names at runs of non-alphanumeric characters, at
lowercase-or-digit to uppercase transitions, and before the final uppercase of
an acronym when it is followed by lowercase text. Lowercase every token. Thus
`accessToken`, `APIKey`, and `x_api_key` become `[access, token]`, `[api, key]`,
and `[x, api, key]`.

A name is sensitive when any token equals `password`, `passwd`, `token`,
`secret`, `authorization`, `cookie`, `credential`, or `bearer`; when the sole
token is `basic`; or when any contiguous token sequence equals `api key`,
`access key`, `private key`, `client secret`, `access token`, or `refresh token`.
Matching tokens rather than arbitrary substrings avoids classifying names such
as `monkey` as keys. Positive acronym/delimiter cases and negative substring
cases are part of the test contract.

Recursively redact keys and flags including:

- `password` and `passwd`
- `token`
- `secret` and `client_secret`
- `bearer` and `basic`
- `authorization`
- `cookie`
- `credential`
- `api_key` and `access_key`
- `private_key`

Handle CLI forms such as `--token VALUE` and `--token=VALUE`. For header-like
strings, parse `Name: Value` and `Name=Value`, canonicalize the name, and redact
the complete value when sensitive. Parse URLs to remove userinfo and redact
query values whose canonical names are sensitive. If a header or URL cannot be
parsed, apply conservative recognizers for authorization prefixes, URL
userinfo, and `name=value` pairs; otherwise preserve the bounded string. Apply
the same sanitizer to diagnostic messages and causes because external errors
can echo credentials.

Default bounds:

- 4 KiB per string value, truncated on a valid UTF-8 boundary;
- 100 entries per array or object;
- nesting depth of 8;
- 64 KiB per complete JSONL record, including the trailing newline.

Truncated values include an explicit marker. After ordinary bounds are applied,
serialize the record and measure UTF-8 bytes including the newline. If a command
record is larger than 65,536 bytes, replace `parameters` with
`{"_truncated":true,"original_bounded_bytes":<u64>}`. For a system record,
replace `context` with the same shape. Shorten diagnostic `message` and `cause`
to 1 KiB each, set `record_truncated: true`, and serialize again.

If the second form is still too large, emit a minimal record. A command fallback
retains every required command envelope field, compact `parameters`, and
`record_truncated:true`. A system fallback retains every required system
envelope field, compact `context`, and `record_truncated:true`. An error fallback
diagnostic retains `code`, a fixed `message:"diagnostic truncated"`, and
`exit_code_hint`; optional identity fields `domain` and `operation` are retained
when they fit. Every stage measures bytes including exactly one trailing newline
and must produce valid JSON no larger than 65,536 bytes. Serialization failure
may drop the event under the best-effort contract.

JSON serialization escapes line breaks and prevents injection of additional
JSONL records.

`AH_LOG_UNREDACTED=1` disables secret substitution only. Size, collection,
depth, and total-record limits remain active. Documentation must warn that this
mode can expose credentials, source code, request bodies, SQL, and shell
arguments.

## Storage And Retention

The default directory is `<global-config-dir>/logs`. Daily filenames use UTC:

```text
aihelper-YYYY-MM-DD.jsonl
```

Compute the date for every write so a long-running MCP process rotates at UTC
midnight. Retain the current UTC date and the preceding nine dates. Cleanup runs
best effort once per observed date per process.

Delete only regular, non-symlink files whose complete names match the expected
date pattern and whose parsed date is older than the retention window. Preserve
future-dated files to avoid deleting a new day's records during concurrent
midnight rollover or clock skew; they age into the normal window as time
advances. Leave unrelated files and malformed names untouched.

Serialize and bound the complete line before opening the file. Open the daily
file in append mode and acquire a short cross-process exclusive lock using
`fs2`. Retry lock acquisition for at most 50 ms. Write the complete buffer with
`write_all`, append a newline, and flush without calling `sync_all` for every
event. Only lock acquisition is bounded to 50 ms; synchronous directory, open,
write, flush, and cleanup operations can still experience filesystem latency.
This tradeoff avoids asynchronous loss at normal CLI shutdown while keeping
lock contention bounded.

Opening, locking, serialization, writing, flushing, permission, and cleanup
errors are swallowed. A process crash releases the OS lock. This is an explicit
best-effort contract: a logger failure can lose an event but cannot fail the
command. Only lock waiting has a strict latency bound; filesystem operations do
not.

## Configuration

Initial configuration surface:

- `AH_LOG_UNREDACTED=1`: disable secret substitution while keeping bounds.
- `AH_CONFIG_DIR`: continues to select the global configuration root and thus
  the log directory.

No new global CLI flags or persistent settings file are introduced initially.
Logging is enabled by default when the global log directory can be resolved.

## Dependencies

- `fs2` for cross-process advisory file locking.
- A small date/time dependency already compatible with the workspace lockfile,
  or an equivalent internal clock abstraction, for UTC filenames, RFC 3339
  timestamps, and ten-date retention.

The clock and event sink must be injectable in tests.

## Test Strategy

### Unit Tests

- recursive JSON key redaction;
- CLI `--key value` and `--key=value` redaction;
- camelCase, acronym, delimiter, contiguous compound, and false-positive name
  normalization cases;
- authorization headers, URL userinfo, and sensitive query parameters;
- diagnostic redaction;
- Unicode-safe string truncation and total record bounds;
- `AH_LOG_UNREDACTED=1` with bounds still active;
- command and system schema serialization;
- warning-level system diagnostic and minimal-fallback schema;
- UTC midnight rollover with an injected clock;
- exact ten-date retention and preservation of unrelated files and symlinks.

### CLI Integration Tests

- successful built-in and host commands;
- command errors with normalized diagnostics;
- parse and configuration errors as system events with no command completion;
- help and version;
- uniform raw argv parameters for host commands, plugins, `mcp.serve`, help,
  and version;
- longest catalog descriptor identity selection plus no-catalog and no-match
  fallback identities;
- dynamic plugin success/error and discovery warnings;
- `run.check` data with `success=false` logged as command success;
- exactly one completion record for every routed command and help/version path;
- no command completion for pre-routing errors, alongside exactly one matching
  system event.

Tests use an isolated `AH_CONFIG_DIR`.

### MCP Integration Tests

- successful tool calls;
- typed command errors;
- unknown tools and invalid context;
- queue, timeout, and cancellation errors;
- response-validation errors;
- JSON-RPC-only stdout with file logging enabled;
- exactly one completion record per tool call;
- `mcp_server` and `mcp_transport` system events;
- normal stdio EOF without a system error;
- completed tool call followed by response-send failure;
- `mcp.serve` construction/transport failure producing one CLI completion and
  one system event without a synthetic tool completion.

### Concurrency And Failure Isolation

- concurrent CLI processes append parseable, non-interleaved JSONL records;
- lock contention remains bounded;
- read-only directories, failed writes, and failed cleanup do not change command
  exit status or output;
- logging never emits to MCP stdout or stderr.
- byte-for-byte CLI stdout/stderr and exit-status comparisons with logging
  enabled, unavailable, lock-contended, and write-failing;
- oversized-record compaction produces one valid bounded JSON record;
- first-stage and minimal fallback records for both command and system schemas,
  with newline-inclusive byte assertions.

## Documentation Changes

Add a logging reference covering location, schema, retention, redaction,
`AH_LOG_UNREDACTED`, best-effort delivery, and the residual risk of opaque
positional secrets. Update MCP and agent recipes to point to the global log
directory for failure diagnosis.
