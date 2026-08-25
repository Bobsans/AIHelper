# Recipe: wire AIHelper into an AI agent

Goal: make an AI coding agent able to call AIHelper, in one command, without
hand-editing that agent's configuration.

Targets: `claude`, `codex`, `gemini`, `cursor`, `copilot`. The server is
registered as `aihelper`.

## Check the current state first

```bash
ah ai status
```

Reports per target whether the agent CLI is on `PATH` (or that the target is
file-backed), whether the MCP server is registered and with which transport,
whether a legacy `ah` registration is still present, and whether the rules block
is installed.

## Preview before changing anything

```bash
ah ai install claude --dry-run
```

Prints the exact agent CLI commands and file changes without running or writing
anything. It also never installs or starts the managed service.

## Install

```bash
ah ai install claude
```

On a terminal with no flags this asks for the scope, the components, and the
transport, then shows a summary and waits for confirmation. In a script or a
pipe it takes the defaults instead: the target's own default scope, both
components, and stdio.

Point the agent at an already running local HTTP server:

```bash
ah ai install codex --transport http
```

Let AIHelper own the server through the Windows managed service, installing or
starting it as needed:

```bash
ah ai install cursor --transport managed
```

Only loopback endpoints are accepted. Add `--url` with `--transport http` when
the server listens on a port other than `8787`.

## Install one half only

```bash
ah ai install claude --rules-only
ah ai install claude --mcp-only
```

`--rules-only` works even when the agent CLI is not installed.

## Remove

```bash
ah ai uninstall claude
```

Removes the MCP registration, including one left under the previous `ah` name,
and strips the rules block, leaving any user-authored content in the rules file
intact.

## Notes for agents

- The rules block deliberately does not copy the manual. Read
  `ah ai info --json` for the authoritative command catalog.
- Re-running install after enabling or disabling plugins refreshes the domain
  list inside the block.
- AIHelper never rewrites the configuration of an agent that ships its own MCP
  CLI; registration is delegated to that CLI. See [`ah ai`](../../reference/ai.md).
