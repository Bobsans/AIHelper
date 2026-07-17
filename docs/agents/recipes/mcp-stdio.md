# Use AIHelper as an MCP stdio server

Start one long-lived server process from the MCP client:

```text
ah --cwd <workspace> --limit 200 mcp serve
```

Do not pass `--json`; stdout is the MCP transport.

## Calling tools

1. List tools and select the narrowest `ah.*` command.
2. Read the impact warning and `dev.aihelper/risk` metadata before calling it.
3. Supply per-call `context.cwd` when the task may target a different
   workspace.
4. Use `context.limit` and `context.timeout_ms` to bound large or slow work.
5. Treat `ah.run.check`, `ah.task.run`, `ah.postgres.exec`, and other
   high/critical-risk tools as arbitrary mutation boundaries.

Example arguments:

```json
{
  "path": "src/lib.rs",
  "from": 1,
  "to": 120,
  "context": {
    "cwd": "D:\\work\\project",
    "limit": 120,
    "timeout_ms": 10000
  }
}
```

Relative paths and child processes use `context.cwd`, not the MCP client's own
working directory.

Plugin state tools (`ah.plugins.enable`, `ah.plugins.disable`, and
`ah.plugins.reset`) change the live catalog. Refresh the client's tool list
after receiving the tool-list-changed notification.

When a tool returns retryable diagnostic `EXECUTOR_DRAINING`, wait briefly and
retry. The previous timed-out handler is still exiting and no new handler will
start until that cleanup completes.
