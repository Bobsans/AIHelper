# Use one local AIHelper MCP HTTP server

On Windows, prefer the idempotent managed lifecycle when the server should
survive terminal closure and start at user logon:

```text
ah mcp service status --json
ah --cwd D:\work\project mcp service install
```

The managed server runs through the sibling windowless
`ah-mcp-service.exe` launcher. The launcher starts `ah.exe` in a kill-on-close
job without creating a console and does not depend on the controller console
after readiness. Non-interactive external tools invoked by MCP commands also
run without creating visible console windows.

`install` records the current `ah.exe`, requires its sibling service launcher,
registers that launcher for the current user, starts it by default, and succeeds
only after exact readiness. Use `--no-start` when an agent is allowed to
reconcile registration but must not change the running process:

```text
ah --cwd D:\work\project mcp service install --no-start --json
```

An agent can safely call `status --json` at any time. It is read-only, does not
wait for a concurrent lifecycle command, and returns exit code `0` for
not-installed, drift, and scheduler-error snapshots. Inspect these fields:

```text
registration.status
registration.diagnostic_code
scheduler.state
runtime.status
readiness.status
lifecycle.status
drift[]
```

Treat `runtime.status=restart_backoff` as launcher-owned retry evidence, not a
native Task Scheduler countdown. Confirm `scheduler.state=running`, retain the
`runtime.diagnostic_code`, and verify that `drift[]` has no
`settings.restart_count` or `settings.restart_interval` entry. Native Scheduler
retries are disabled; the launcher allows three retries after the initial child
launch with `PT1M` spacing. `scheduler.last_result` is not an attempt counter.
Do not infer attempts remaining or a next-retry timestamp from status.

Use `ah mcp service start --json` only after registration is `installed` and
drift is empty. Success means readiness matched the durable service,
configuration, version, PID, and instance UUID; Task Scheduler submission alone
is not considered success.

To stop an installed service, call status first and then use:

```text
ah mcp service stop --json
```

Treat `already_stopped`, `stopped`, and `forced_stopped` as successful terminal
actions. AIHelper sends control shutdown only to the exact readiness identity;
its fallback can target only a revalidated owned Scheduler instance with an
exact Scheduler UUID and durable PID, or one exact process-free queued retry.
`MCP_SERVICE_STOP_UNSAFE` means the identity proof was insufficient and no
unknown process was stopped. `MCP_SERVICE_STOP_TIMEOUT` means a mutation was
attempted but complete quiescence was not proven.

Restart only an installed, drift-free registration:

```text
ah mcp service restart --json
```

Success requires a new process instance UUID. If restart fails after stop, leave
the registration in place, inspect `status --json`, and retry `start` or
`restart` according to the observed state; do not try to recover the old PID.

When removal is intended, use the supported idempotent cleanup:

```text
ah mcp service uninstall --json
```

Uninstall conditionally deletes only the owned task and verified semantic state.
It preserves the executable, plugins, configuration, logs, unexpected files,
and lock anchors. A foreign task or unproven occupied orphan blocks cleanup. An
`already_uninstalled` result can have `null` service, configuration, and endpoint
identities.

Do not delete or rewrite files below
`%LOCALAPPDATA%\AIHelper\managed-mcp` to recover from an error. Preserve the
stable diagnostic and let a later lifecycle command perform supported
reconciliation.

For a foreground process or on non-Windows platforms, use manual serve:

Start one foreground process:

```text
ah mcp serve --transport http --port 8787 --max-active 32
```

Connect clients to `http://127.0.0.1:8787/mcp`.

To create or replace a vault record without exposing values to MCP, keep this
HTTP process running and use:

```text
ah secrets add billing --kind postgres --open
ah secrets edit billing --open
```

The CLI prints a protected setup URL on the same loopback process and makes a
best-effort attempt to open it. In a headless session, copy the printed URL into
a browser that can reach that origin. The CLI rejects a returned URL unless it
uses the configured origin, exact `/secrets/setup` path, and exactly one
non-empty `capability` query parameter. The random 256-bit capability expires
after ten minutes and is consumed only by a successful POST; GET, POST, expiry,
and reuse failures return the stable setup diagnostic. The server does not log
the capability query or form body and returns only `id`, `kind`, `label`, and
nullable `description`. Treat the printed URL as sensitive. For a non-default port, set
`AH_MCP_HTTP_URL=http://127.0.0.1:PORT` for the `ah secrets ... --open` command.

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

Commands that resolve project-relative paths, and every `ah.job.start` target,
must include an absolute `context.cwd`. Stateless commands may use the server
default, but callers should provide `cwd` whenever a relative path is involved:

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

Before calling a tool with credential slots, read its generated description for
accepted kinds. When the ID is unknown, call `ah.secrets.list` with the stated
kind filter, then pass only the selected ID in `credentials.database` or
`credentials.basic`. Do not cache IDs from `tools/list`: live IDs are
intentionally absent. `/mcp` never accepts or returns secret values.

`postgres --password-env` remains a direct CLI compatibility path. It is not a
solution for HTTP MCP; use `credentials.database` so the long-running server
resolves the vault value internally.

The listener is loopback-only and validates Host and Origin, but it has no
authentication. Treat access by any local process running as the same user as
full access to every published AIHelper tool.
