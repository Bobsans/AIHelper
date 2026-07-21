# Use AIHelper as an MCP stdio server

Configure the MCP client to own one subprocess:

```text
ah --cwd <workspace> --limit 200 mcp serve --max-active 32
```

Do not pass `--json`; stdout is reserved for MCP protocol messages.

## Calling tools

1. Select the narrowest `ah.*` command and inspect its risk metadata.
2. Supply `context.cwd` when a call targets a directory other than the startup
   workspace.
3. Use `context.limit` and `context.timeout_ms` to bound large or slow work.
4. Use `ah.job.start` when the client should not keep one `tools/call` open.
5. Poll `ah.job.result`; it always returns immediately.

Direct calls and jobs share 32 execution slots by default. Commands execute in
parallel without an AIHelper queue. Retry `EXECUTION_CAPACITY_FULL` only when a
slot is likely to have become available; rejected work never starts later.

Cancellation and timeout complete logically at once. `draining: true` means the
plugin has not physically returned and still owns one slot, but unrelated calls
continue in other slots.

Plugin state tools change the shared live catalog. Refresh the tool list after a
tool-list-changed notification. This notification is also broadcast when a
detached `ah.job.start` target changes plugin state at completion.

Completed MCP calls and transport problems are written to the normal daily JSONL
logs. `queue_wait_ms` is always zero; timed-out execution reports
`timeout_phase: "execution"`.
