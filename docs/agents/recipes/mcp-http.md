# Use one local AIHelper MCP HTTP server

Start one foreground process:

```text
ah mcp serve --transport http --port 8787 --max-active 32
```

Connect clients to `http://127.0.0.1:8787/mcp`.

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
