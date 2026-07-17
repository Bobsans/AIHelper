# `ah mcp`

AIHelper can serve its typed command catalog as a Model Context Protocol server.
The only supported transport is stdio.

## `ah mcp serve`

```text
ah [--cwd PATH] [--limit N] mcp serve \
  [--max-queued N] \
  [--default-timeout-ms MILLISECONDS]
```

Defaults:

- `--max-queued 32`
- `--default-timeout-ms 300000`
- default working directory: the directory in which `ah` starts, or global
  `--cwd`
- default result limit: global `--limit`, when supplied

`--json` is rejected for this command because stdout is reserved exclusively
for MCP protocol messages. Logs and diagnostics must not be written to stdout.
HTTP, SSE, and Streamable HTTP transports are not supported.

## Client configuration

Configure the MCP client to start the `ah` executable directly:

```json
{
  "mcpServers": {
    "aihelper": {
      "command": "C:\\tools\\aihelper\\ah.exe",
      "args": [
        "--cwd",
        "D:\\work\\project",
        "--limit",
        "200",
        "mcp",
        "serve"
      ]
    }
  }
}
```

On Unix-like systems, use the corresponding absolute executable and workspace
paths.

## Tool names

Each typed command is exposed as a separate tool:

```text
ah.<domain>.<command>
```

Examples:

- `ah.file.read`
- `ah.search.text`
- `ah.git.tag.create`
- `ah.github.issue.create`
- `ah.postgres.exec`
- `ah.run.check`
- `ah.plugins.disable`

`ah.mcp.serve` is intentionally not exposed because invoking it from the server
would recursively start another stdio server. Dynamic plugin tools appear only
when the corresponding shared library is loaded and its domain is enabled.

Use `ah.plugins.list` to inspect `mcp_exposed` and
`mcp_omission_reason`. Enabling, disabling, or resetting a plugin domain from an
MCP tool updates the live tool catalog and emits a tool-list-changed
notification.

## Per-call execution context

Every tool input includes an optional reserved `context` object:

```json
{
  "context": {
    "cwd": "D:\\work\\project",
    "limit": 100,
    "timeout_ms": 30000
  }
}
```

- `cwd` resolves relative paths and is the working directory for child
  processes.
- `limit` caps supported line or item collections.
- `timeout_ms` covers both time in the queue and command execution.

Omitted fields use the server defaults. `context` is removed before validating
and invoking the domain command.

## Safety metadata

All tools include standard MCP annotations:

- `readOnlyHint`
- `destructiveHint`
- `idempotentHint`
- `openWorldHint`

The description includes a human-readable impact warning and risk level.
Machine-readable details are available under:

```text
_meta["dev.aihelper/risk"]
```

The object contains `level`, `impact`, `effects`, and `reversibility`. Metadata
is conservative: a command is marked for the most consequential behavior its
input can request. For example, PostgreSQL read tools warn that
`ensure_tool=true` can download a shared toolchain, and `postgres.explain` is
destructive because `analyze=true` executes the supplied SQL.

AIHelper's `task.*` tools are ordinary tools for saved shell recipes. MCP's
protocol-level Tasks capability is explicitly marked unsupported for every
tool.

## Results and errors

Successful calls return:

- `structuredContent`: the validated command output object
- a compact text summary
- execution metadata under `_meta["dev.aihelper/execution"]`

Operational failures are tool results with `isError=true` and a structured
diagnostic under `_meta["dev.aihelper/diagnostic"]`. Only protocol problems,
such as an unknown tool name, use MCP protocol errors.

`EXECUTOR_DRAINING` is a retryable admission error. It means a timed-out
handler has not exited yet, so the sequential executor is refusing new work to
preserve its no-overlap guarantee.

## Scheduling and cancellation

The current executor is a bounded, sequential FIFO queue. Command handlers do
not overlap, which protects process-wide state and existing plugins while the
parallel scheduling policy is developed.

The executor boundary is independent from MCP and can later be replaced by a
resource-aware parallel implementation without changing tool names or schemas.
Queued calls can be cancelled immediately. Active cancellation is propagated
to domains that support interruption; process tools terminate their process
groups, while blocking third-party operations remain bounded by their request
timeouts. If a handler ignores cancellation after its timeout, the server keeps
draining that handler and rejects new calls with `EXECUTOR_DRAINING` until it
exits. Protocol request IDs may be reused after completion; each call receives
a distinct internal execution ID so late cleanup cannot affect a newer call.
