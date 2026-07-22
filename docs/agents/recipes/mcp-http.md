# Use one local AIHelper MCP HTTP server

Start one foreground process:

```text
ah mcp serve --transport http --port 8787 --max-active 32
```

Connect clients to `http://127.0.0.1:8787/mcp`.

Check the exact running process without creating an MCP session:

```text
GET http://127.0.0.1:8787/health/ready
```

The JSON response contains `status`, the binary `version`, the process `pid`,
and a process-lifetime UUID v4 `instance_id`. A restart always creates a new
identity.

To stop that exact process, first read readiness and then send its current
identity to the control endpoint:

```text
POST http://127.0.0.1:8787/control/shutdown
Content-Type: application/json
```

```json
{
  "instance_id": "550e8400-e29b-41d4-a716-446655440000"
}
```

The server accepts a match with HTTP `202` and
`{"status":"shutting_down","instance_id":"..."}`. A stale identity returns
HTTP `409` with `INSTANCE_ID_MISMATCH` and leaves the current process running.
Do not reuse an identity across restarts. The endpoint has the same loopback,
Host, Origin, and no-CORS policy as MCP and readiness; it has no separate
authentication.

After an accepted shutdown, wait for the process. Exit code `0` means lifecycle
completion stayed within the shared five-second budget. Exit code `1` with
`MCP_SHUTDOWN_TIMEOUT` means draining exceeded that budget; startup, bind, or
unexpected transport failures also exit with code `1`. The HTTP `202` response
alone is not a final success signal.

Claude Code:

```text
claude mcp add --transport http aihelper http://127.0.0.1:8787/mcp
```

Codex `config.toml`:

```toml
[mcp_servers.aihelper]
url = "http://127.0.0.1:8787/mcp"
```

OpenCode configuration:

```json
{
  "mcp": {
    "aihelper": {
      "type": "remote",
      "url": "http://127.0.0.1:8787/mcp",
      "enabled": true
    }
  }
}
```

Every direct command and every `ah.job.start` target must include an absolute
`context.cwd`. This is mandatory because different sessions can work in
different repositories:

```json
{
  "path": "src/lib.rs",
  "context": {
    "cwd": "D:\\work\\project-a",
    "timeout_ms": 10000
  }
}
```

Sessions share the plugin catalog, execution capacity, jobs, and retained
results. A job started by one client can be inspected or cancelled by another
client that knows its `job_id`. JSON-RPC request IDs and direct cancellation stay
isolated per session.

The listener is loopback-only and validates Host and Origin, but it has no
authentication. Treat access by any local process running as the same user as
full access to every published AIHelper tool.
