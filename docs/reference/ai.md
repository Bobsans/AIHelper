# `ah ai`

AI-agent focused utility commands.

## `ah ai info`

Print full command manual aggregated from built-in and dynamic plugin manuals.

```bash
ah ai info [--domain <domain>] [--json]
```

Flags:
- `--domain <domain>`: show manual only for one domain (example: `file`, `search`)
- `--json`: emit structured machine-readable manual

Output includes:
- global CLI flags
- host commands (`ai info`, plugin management, and secret-vault management/discovery)
- per-plugin command descriptions
- per-command examples intended for AI agents

Interactive text output uses semantic colors for section headings, domains,
flags, command usages, and examples. Colors are disabled automatically for
pipes, redirects, captured output, and JSON mode. Set `NO_COLOR` to disable
colors explicitly.

Notes:
- plugin examples are stored in plugin source code and validated by tests
- dynamic plugins may optionally provide manual via `ah_plugin_manual_json_v1`
- agents discover redacted secret IDs through typed/MCP `secrets.list`; secret values never appear in `ah ai info`
- warning labels emitted by host and domain commands use the shared text formatter

## `ah ai install`

Wire AIHelper into an AI coding agent: register the MCP server and install a
managed rules block that teaches the agent to read `ah ai info`.

```bash
ah ai install <TARGET> [--scope <local|project|user>] [--transport <stdio|http|managed>] [--url URL] [--mcp-only | --rules-only] [--yes] [--dry-run]
```

The registered server name is `aihelper` in every target.

Supported targets:

| target | scopes | default scope | registrar | rules file |
| --- | --- | --- | --- | --- |
| `claude` | `local`, `project`, `user` | `local` | `claude mcp` | `CLAUDE.md` |
| `codex` | `project`, `user` | `project` | `codex mcp` | `AGENTS.md` |
| `gemini` | `project`, `user` | `project` | `gemini mcp` | `GEMINI.md` |
| `cursor` | `project`, `user` | `project` | JSON file | `.cursor/rules/ah.mdc` |
| `copilot` | `project` | `project` | JSON file | `.github/copilot-instructions.md` |

Project and local scopes place files next to the repository; the user scope
places them in the agent's home directory (`~/.claude/`, `~/.codex/`,
`~/.gemini/`, `~/.cursor/`).

### Registration is delegated when the agent has a CLI

AIHelper never parses or rewrites the configuration of an agent that owns a CLI
for the job. Those files routinely hold credentials for unrelated MCP servers,
comments, and per-tool settings. `claude`, `codex`, and `gemini` are registered
by running their own `mcp add`, so the agent stays the only writer.

Consequences:

- the agent CLI must be on `PATH`. Otherwise the command fails with
  `AI_AGENT_CLI_MISSING`; `--rules-only` still works;
- how the agent formats its own file is the agent's decision. `codex mcp add`
  rewrites `~/.codex/config.toml` and does not preserve leading comments;
- presence checks stay read-only. Claude is probed by reading its JSON config,
  Codex through `codex mcp list --json`, Gemini by reading `settings.json`.

`cursor` ships only an editor launcher and Copilot has no CLI, so their JSON
configurations are merged directly: AIHelper replaces only its own entry and
preserves every other server and unrelated key. A file that exists but does not
parse fails with `AI_CONFIG_UNPARSABLE` and is left untouched.

Codex keeps MCP servers in a single global list, so `--scope project` still
registers the server in the user scope and reports a warning. The rules file
follows the requested scope.

### Transports

- `--transport stdio` (default) registers the absolute path of the running `ah`
  executable plus `mcp serve`. A bare `ah` is never written, because agents
  spawn MCP servers without a guaranteed `PATH`.
- `--transport http` registers a loopback endpoint, `http://127.0.0.1:8787/mcp`
  by default, or `--url` when the server runs on another port. AIHelper probes
  `/health/ready` first and warns, without failing, when nothing answers.
- `--transport managed` uses the endpoint of the Windows managed service,
  installing or starting it when necessary. It is not a third wire format: the
  agent still receives an HTTP entry, and only the ownership of the process
  differs. `--url` cannot be combined with it, because the service owns its
  endpoint.

Only `127.0.0.1`, `::1`, and `localhost` are accepted: the MCP server has no
authentication and no TLS. See [`ah mcp`](mcp.md).

### The managed transport

The endpoint always comes from `ah mcp service status`, never from a hardcoded
port, so a service installed on a non-default port produces a correct entry.

| managed state | result |
| --- | --- |
| ready | the endpoint is written as it is |
| installed but stopped | the service is started, then written |
| not installed | the service is installed and started, then written |
| configuration drift or scheduler error | `AI_MANAGED_NOT_HEALTHY`, naming the underlying `MCP_SERVICE_*` code |

A drifted service is reported rather than repaired; inspect it with
`ah mcp service status`. Managed lifecycle commands are Windows-only, so
`--transport managed` fails with `AI_MANAGED_UNSUPPORTED` elsewhere while
`--transport http` stays available. `--dry-run` classifies and refuses but never
installs or starts anything.

### Interactive and non-interactive use

Run on a terminal with no decision flags, `ah ai install <TARGET>` asks: the
scope, which components to install, the transport, and, for a manual HTTP
endpoint, the URL. It then prints every command it will run and every file it
will write, and does nothing until that summary is confirmed. Declining changes
nothing and is not an error.

The managed item carries live state in its label (`running`,
`installed, stopped; will be started`, `will be installed`) and is absent when
the service is unavailable or needs repair.

Prompting is skipped entirely when standard input is not a terminal, or when any
of `--scope`, `--transport`, `--url`, `--mcp-only`, `--rules-only`, `--yes`,
`--dry-run`, `--json`, or `--quiet` is present. In that case the defaults are
the target's own default scope, both components, and `transport = stdio`.

The managed service is therefore never installed implicitly. Registering a
per-user Task Scheduler task is a persistent, system-level side effect and
requires either the interactive selection or an explicit `--transport managed`,
which is honored in scripts. Use `--yes` to keep the prompt defaults without the
confirmation step.

### Idempotency and the previous server name

Before registering, AIHelper checks what the agent already has:

- absent: the server is registered;
- identical: the command reports `unchanged` and runs nothing;
- different: the previous entry is reported, then replaced with `mcp remove`
  followed by `mcp add`.

Releases before the `aihelper` name registered the server as `ah`. Both
`install` and `uninstall` remove such a registration and say so, so an upgrade
does not leave the agent holding two identical servers. `ah ai status` reports a
legacy registration when it finds one.

The rules block is delimited by `<!-- ah:begin ... -->` and `<!-- ah:end -->`.
A re-run replaces the block in place and leaves user-authored content in the
file untouched. An unbalanced marker pair fails with
`AI_RULES_BLOCK_MALFORMED` instead of guessing. Writes are atomic and parent
directories are created as needed.

`--dry-run` reports the exact commands and file changes without running or
writing anything.

## `ah ai uninstall`

```bash
ah ai uninstall <TARGET> [--scope <local|project|user>] [--dry-run]
```

Removes the MCP registration, including one left under the previous `ah` name,
and strips the managed rules block. A rules file that becomes empty is deleted;
a file that still holds user content is rewritten without the block. Both halves
are idempotent and report `not present` when there is nothing to remove.

## `ah ai status`

```bash
ah ai status [TARGET] [--json]
```

Reports, per target: whether the agent CLI is available, or that the target is
file-backed; whether the MCP server is registered and with which transport;
whether a legacy registration is still present; and whether the rules block is
installed. It performs no mutation.

## Integration output and diagnostics

`--json` emits schema version 1 with `command`, `schema_version`, `target`,
`scope`, `changed`, `dry_run`, `mcp`, `rules`, `managed_service`, and
`warnings`. Component actions are `installed`, `updated`, `unchanged`,
`removed`, `not_present`, and `skipped`. `mcp.registrar` is `cli` or `file`;
`mcp.path` is set only for file-backed targets. `managed_service` is `null`
unless the managed transport was used, and otherwise carries `already_running`,
`started`, or `installed`. `--quiet` suppresses successful output on both
channels; warnings still reach stderr.

Diagnostics:

- `AI_TARGET_UNKNOWN`
- `AI_TARGET_SCOPE_UNSUPPORTED`
- `AI_AGENT_CLI_MISSING`
- `AI_AGENT_CLI_FAILED`
- `AI_MANAGED_UNSUPPORTED`
- `AI_MANAGED_NOT_HEALTHY`
- `AI_MANAGED_ENDPOINT_UNKNOWN`
- `AI_NOTHING_SELECTED`
- `AI_CONFIG_UNPARSABLE`
- `AI_RULES_BLOCK_MALFORMED`
- `AI_URL_NOT_LOOPBACK`
- `AI_PROMPT_FAILED`
