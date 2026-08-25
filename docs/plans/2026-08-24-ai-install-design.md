# `ah ai install` Agent Integration Design

**Date:** 2026-08-24
**Status:** Implemented

## Goal

Give users one command that wires AIHelper into an AI coding agent:

1. register the AIHelper MCP server in that agent's MCP configuration;
2. install a short rules block that teaches the agent how to reach `ah`
   commands and where the authoritative manual lives.

The command is interactive when run without flags on a terminal, and fully
scriptable when any decision is supplied as a flag.

## Scope

In scope: MCP registration and rules installation for a fixed table of agent
targets, install/uninstall/status symmetry, managed-service detection and
opt-in provisioning, idempotent registration that never damages a user's
existing agent configuration.

Out of scope: repairing a drifted managed service (report and defer to
`ah mcp service`), auto-detecting which agents are installed, an `--all`
target, per-agent skill or command scaffolding, remote or non-loopback MCP
endpoints, OAuth or bearer-token server configurations.

## Command Surface

```text
ah ai install <target> [--scope <project|user|local>]
                       [--transport <stdio|http|managed>]
                       [--url URL]
                       [--mcp-only | --rules-only]
                       [--dry-run] [--yes] [--json] [--quiet]

ah ai uninstall <target> [--scope <project|user|local>] [--dry-run] [--json]
ah ai status [<target>] [--json]
```

`ah ai status` reports, per target: whether the agent CLI is available,
whether the MCP entry exists and which transport it points at, whether the
managed rules block exists, and the file paths involved. It performs no
mutation.

`uninstall` exists because the command changes state the user owns; every
mutation must be reversible by the same tool.

## Registration Backends

The important finding from environment verification: three of the five targets
ship a first-class CLI for MCP registration. Delegating to it removes any need
for AIHelper to parse or rewrite the agent's own configuration format.

This matters concretely. A real `~/.codex/config.toml` contains comments,
mixed quoting styles, nested `[mcp_servers.*.env]` tables, per-tool
`[mcp_servers.*.tools.*]` approval tables, and plaintext credentials for other
servers. AIHelper must never round-trip such a file. `codex mcp add` owns that
responsibility and is the only writer.

Verified against `codex-cli 0.146.0`, `claude 2.1.220`, and the Gemini CLI:

| target | registrar | add command | remove | read probe |
| --- | --- | --- | --- | --- |
| `claude` | CLI | `claude mcp add [-s SCOPE] [-t stdio\|http] ah ...` | `claude mcp remove [-s SCOPE] ah` | read its JSON config |
| `codex` | CLI | `codex mcp add ah (--url URL \| -- CMD...)` | `codex mcp remove ah` | `codex mcp list --json` |
| `gemini` | CLI | `gemini mcp add [-s SCOPE] [-t stdio\|http] ah ...` | `gemini mcp remove ah` | `gemini mcp list` |
| `cursor` | file | `.cursor/mcp.json` / `~/.cursor/mcp.json`, key `mcpServers` | — | — |
| `copilot` | file | `.vscode/mcp.json`, key `servers` | — | — |

`cursor` ships only an editor launcher (`cursor.exe`, no `mcp` subcommand) and
Copilot has no CLI at all, so those two are written as JSON. `serde_json` is
already a direct dependency; no new crate is required for either backend.

If a CLI-backed target's binary is not on `PATH`, registration fails with
`AI_AGENT_CLI_MISSING`. AIHelper does not fall back to editing that agent's
config file. An agent that is not installed does not need an MCP server
registered, and the fallback would reintroduce exactly the round-trip risk
this design avoids. `--rules-only` still works in that situation.

A failing agent CLI surfaces as `AI_AGENT_CLI_FAILED` with the child's exit
code and captured stderr.

## Target Table

Everything agent-specific is one static table. No plugin mechanism, no dynamic
discovery.

| target | scopes | http | rules file |
| --- | --- | --- | --- |
| `claude` | `local` (default), `project`, `user` | yes | `CLAUDE.md` |
| `codex` | `project` (default), `user` | yes | `AGENTS.md` |
| `cursor` | `project` (default), `user` | yes | `.cursor/rules/ah.mdc` |
| `copilot` | `project` only | yes | `.github/copilot-instructions.md` |
| `gemini` | `project` (default), `user` | yes | `GEMINI.md` |

Every target supports HTTP. Codex accepts `url = "..."` under
`[mcp_servers.<name>]` and exposes it as `codex mcp add --url`.

AIHelper's `--scope` maps directly onto each agent's own scope vocabulary. A
scope the target does not offer fails with `AI_TARGET_SCOPE_UNSUPPORTED` and
names the scopes it does offer. `local` is accepted only for `claude`, whose
default it is.

The registered server name is `aihelper` in every target.

Earlier development builds used `ah`. That name is kept in
`LEGACY_SERVER_NAMES`, and both `install` and `uninstall` remove a registration
found under it, reporting that they did. Without this, an upgrade would leave
the agent holding two identical servers — the exact duplicate state the rename
was meant to end. `ah ai status` surfaces a legacy registration as
`legacy_server`.

Codex keeps MCP servers in a single global list. `--scope project` is still
accepted there because the rules file legitimately belongs next to the
repository; the registration lands in the user scope and the command reports a
warning saying so.

## Transport and Provisioning Model

The agent configuration only ever expresses two transports. Managed is a
*provisioning* choice layered under HTTP, not a third wire format:

- `transport = stdio` — absolute `current_exe()` path plus `["mcp", "serve"]`.
- `transport = http` — `url` pointing at a loopback `/mcp` endpoint.
- `provision = none | managed` — `managed` means AIHelper owns the process
  through `ah mcp service`.

`--transport managed` is sugar for `transport = http, provision = managed`,
because that is how users think about it. The interactive menu presents three
items for the same reason.

Resulting invocations, with `EXE` the absolute `current_exe()` path:

```text
claude mcp add -s project aihelper -- EXE mcp serve
claude mcp add -s project -t http aihelper http://127.0.0.1:8787/mcp
codex  mcp add aihelper -- EXE mcp serve
codex  mcp add aihelper --url http://127.0.0.1:8787/mcp
gemini mcp add -s project aihelper EXE mcp serve
gemini mcp add -s project -t http aihelper http://127.0.0.1:8787/mcp
```

The three CLIs disagree on argument shape and the difference is not cosmetic.
Claude and Codex separate the spawned command with `--`; Gemini takes it as a
positional argument and would treat a `--` as an argument to the server.

File-backed targets receive:

```jsonc
// cursor: .cursor/mcp.json — key "mcpServers"
// copilot: .vscode/mcp.json — key "servers"
{ "ah": { "command": "C:\\path\\to\\ah.exe", "args": ["mcp", "serve"] } }
{ "ah": { "type": "http", "url": "http://127.0.0.1:8787/mcp" } }
```

Managed lifecycle commands are Windows-only (`src/mcp_service/mod.rs`). On
other platforms the managed menu item is absent and `--transport managed`
fails with `AI_MANAGED_UNSUPPORTED`. Plain `--transport http` stays available
everywhere.

The absolute executable path is `std::env::current_exe()`. Agents spawn MCP
servers without a guaranteed `PATH`, so a bare `ah` must never be written.

### Locating the agent CLI on Windows

Agent CLIs are usually npm shims. On Windows `codex` and `gemini` exist only as
`codex.cmd` / `gemini.cmd`; `std::process::Command` searches `PATH` for the bare
name and `.exe` alone and reports `NotFound` for them. Every agent CLI is
therefore resolved through `PATH` × `PATHEXT` before spawning, and an
unresolvable name becomes `AI_AGENT_CLI_MISSING`. Without this, `codex` looks
uninstalled on a machine where it works.

## Idempotency

Before registering, AIHelper runs the target's read-only probe. Two behaviours
found during implementation decide which probe each target gets:

- `claude mcp get <name>` exits `0` whether or not the server exists, and it
  health-checks every configured server, which can block for tens of seconds.
  Neither its exit code nor its latency is usable, so Claude is probed by
  **reading** its JSON configuration: `<project>/.mcp.json` for the project
  scope, `~/.claude.json` for the user scope, and
  `~/.claude.json` → `projects[<root>].mcpServers` for the local scope. Claude
  records the project key with whichever path separator was current when the
  project was first opened, so both spellings are matched. Reading is safe;
  only mutation is delegated.
- `codex mcp list --json` returns a clean structured listing with no health
  check, so Codex uses it directly.
- Gemini, Cursor, and Copilot are probed by reading their JSON configuration.
  The HTTP key is not uniform: Cursor and Copilot use `url`, while Gemini writes
  Streamable HTTP as `httpUrl` and reserves `url` for SSE. The reader accepts
  both; the writer, used only for the two file-backed targets, emits
  `{"type": "http", "url": ...}`.

Three outcomes:

- **absent** — register.
- **present and identical** — report `unchanged`, run nothing.
- **present and different** — interactive mode shows the current entry and
  asks to replace; non-interactive mode replaces and emits a warning naming
  the previous transport. Replacement is `mcp remove` followed by `mcp add`,
  because the agent CLIs do not all treat `add` over an existing name as an
  update.

## Managed Service Detection

Detection reads `ah mcp service status` state — never a hardcoded port. The
endpoint written into the agent config is the `endpoint` field from that
status, so a service installed on a non-default port produces a correct
config.

| status | menu label | action on selection |
| --- | --- | --- |
| readiness ready | `running` | write url only |
| stopped / `restart_backoff` | `installed, stopped` | `service start`, then write url |
| `MCP_SERVICE_NOT_INSTALLED` | `will be installed` | `service install` (starts), then write url |
| configuration drift / scheduler error | `needs repair` | refuse with `AI_MANAGED_NOT_HEALTHY`, print the underlying `diagnostic_code` and point at `ah mcp service status` |

Managed provisioning goes through the existing lifecycle entry points so the
lifecycle lock, ownership marker, and readiness proofs stay authoritative.
`ah ai install` adds no second path to Task Scheduler.

For manual `--transport http` the URL defaults to the managed `endpoint` when
one is known, otherwise `http://127.0.0.1:8787/mcp`. Before registering,
AIHelper probes `GET <origin>/health/ready`. A failed probe is a warning, not
an error: interactive mode asks for confirmation, non-interactive mode
registers and warns that the agent will see a refused connection until a
server is started.

Only loopback URLs are accepted. A non-loopback `--url` fails with
`AI_URL_NOT_LOOPBACK`; the server has no authentication or TLS.

## Interactive Flow

Interactive mode requires a terminal and no decision flags. Steps, in order:

1. **Scope** — `Select` over the scopes the target supports, using that
   agent's own default. Skipped when the target supports exactly one scope
   (`codex`, `copilot`).
2. **Components** — `MultiSelect`, both checked by default: `MCP server`,
   `rules`. Deselecting both fails with `AI_NOTHING_SELECTED` rather than
   silently succeeding.
3. **Transport** — shown when the MCP component is selected. Items are
   filtered by platform, with the managed label taken from live detection:

   ```text
   > Managed service (http://127.0.0.1:8787/mcp) — running
     HTTP endpoint (manual)
     stdio (agent spawns `ah mcp serve`)
   ```

4. **URL** — only for manual HTTP. `Input` with the default described above,
   followed by the readiness probe.
5. **Summary and confirmation** — `Confirm`, listing every command that will
   be run and every file that will be created or modified, the resulting
   transport, and whether the managed service will be installed or started.
   Nothing is executed or written before this is accepted.

Prompts use `dialoguer`, already a direct dependency (`src/commands/secrets.rs`).

## Non-Interactive Rules

Non-interactive when `!std::io::stdin().is_terminal()`, or when any of
`--scope`, `--transport`, `--url`, `--mcp-only`, `--rules-only`, `--yes`,
`--json`, `--quiet` is present.

Defaults in that mode: the target's own default scope, both components,
`transport = stdio`.

The managed service is never installed or started implicitly. Registering a
per-user Task Scheduler task is a persistent, system-level side effect and
requires explicit consent — either the interactive selection or an explicit
`--transport managed`, which is honored in scripts.

`--dry-run` never touches the service, never runs an agent CLI mutation, and
never writes a file. It prints the exact commands and file changes it would
perform, including `would install managed service`.

## Rules and File Mutation Contract

**Rules live inside a managed block:**

```text
<!-- ah:begin (managed by `ah ai install`) -->
...
<!-- ah:end -->
```

This mirrors an established convention; the same repository's own agent files
already carry third-party managed blocks in that shape.

A re-run replaces the block in place, so user-authored content in
`CLAUDE.md` / `AGENTS.md` survives. `uninstall` removes exactly the block and
its surrounding blank line, and deletes the file only if AIHelper created it
and nothing else remains. A file with an opening marker and no closing marker
fails with `AI_RULES_BLOCK_MALFORMED` rather than guessing.

**File-backed MCP configs are merged, never rewritten.** For `cursor` and
`copilot`, AIHelper reads the existing JSON, replaces only its own `ah` entry,
and preserves every other server and unrelated key. A missing file is created
with only the `ah` entry. A file that exists but does not parse fails with
`AI_CONFIG_UNPARSABLE` and is left untouched.

All writes are atomic: write a sibling temporary file, then rename. Parent
directories (`.cursor/rules/`, `.github/`, `.vscode/`) are created as needed.
No backup files are produced — project files are under version control, and
user-level files are protected by the atomic write.

Agent config contents are never echoed to stdout, stderr, or the invocation
log. Those files legitimately contain credentials for unrelated MCP servers.
Only the server name, transport, and resolved path are reported.

## Rules Block Content

The block is roughly 25 lines and deliberately does **not** duplicate the
manual, which would rot on every plugin change. It states:

- AIHelper is available as MCP tools (`ah.*`) and as the `ah` CLI;
- the authoritative manual is `ah ai info --json`, narrowable with `--domain`;
- the list of available domains, generated at write time from
  `PluginManager::collect_plugin_manuals()` so it cannot disagree with the
  installed plugin set;
- prefer `ah` over ad-hoc shell for search, git context, project detection,
  and checked command execution;
- secrets are exposed only as redacted identifiers through `secrets.list`;
  values are never readable by the agent and must not be requested.

## Output Contract

Text output uses the existing `TextFormatter` styles. JSON output is
deterministic and versioned, mirroring the managed-service mutation schema:

```json
{
  "command": "ai.install",
  "schema_version": 1,
  "target": "claude",
  "scope": "project",
  "changed": true,
  "dry_run": false,
  "mcp": {
    "action": "installed",
    "registrar": "cli",
    "transport": "http",
    "url": "http://127.0.0.1:8787/mcp",
    "path": null
  },
  "rules": { "action": "updated", "path": "CLAUDE.md" },
  "managed_service": { "action": "installed", "endpoint": "http://127.0.0.1:8787/mcp" },
  "warnings": []
}
```

Component actions are `installed`, `updated`, `unchanged`, `skipped`.
`registrar` is `cli` or `file`; `path` is `null` for CLI-backed targets.
`managed_service` is `null` when managed provisioning was not involved.
`ai.uninstall` uses the same field order with `removed`, `not_present`.
`--quiet` suppresses successful output on both channels.

## Diagnostics

- `AI_TARGET_UNKNOWN`
- `AI_TARGET_SCOPE_UNSUPPORTED`
- `AI_AGENT_CLI_MISSING`
- `AI_AGENT_CLI_FAILED`
- `AI_MANAGED_UNSUPPORTED`
- `AI_MANAGED_NOT_HEALTHY`
- `AI_NOTHING_SELECTED`
- `AI_CONFIG_UNPARSABLE`
- `AI_RULES_BLOCK_MALFORMED`
- `AI_URL_NOT_LOOPBACK`
- `AI_PROMPT_FAILED`

## Dependencies

None added. CLI delegation removes the TOML round-trip problem entirely, so no
TOML crate is needed. `serde_json` covers the two file-backed targets and
`dialoguer` covers the prompts; both are already direct dependencies.

## Implementation Slices

All four shipped.

1. Target table, scope mapping, CLI registrar with probe/add/remove, rules
   managed block, atomic writes; `claude` and `codex`;
   `install` / `uninstall` / `status`; `--dry-run`; full non-interactive flag
   surface. `--transport http` and `--url` shipped here too, since once the
   invocation builder existed they cost only an argument vector.
2. Managed-service detection, the `managed` transport, the HTTP readiness
   probe, and drift refusal.
3. Interactive prompts and the confirmation summary.
4. File registrar plus `cursor` and `copilot`; `gemini` as a third CLI row;
   the `aihelper` rename with legacy-registration cleanup.

### Ordering inside install

The confirmation prompt is only meaningful if nothing has happened before it.
Install therefore runs in two halves: classification, path resolution, managed
detection, and the readiness probe are all read-only and happen first; agent CLI
mutations, file writes, and managed provisioning happen only after the summary
is accepted. Declining returns a report with every action `skipped` and
`changed: false` — a decision, not an error.

### Known agent behaviour, not a defect

`codex mcp add` rewrites `~/.codex/config.toml` through its own serializer and
does not preserve leading comments. That is the agent's decision about its own
file. It is exactly the class of damage AIHelper would otherwise have to risk
by round-tripping the file itself, which is why registration is delegated.

## Tests

Unit tests (in-crate, no agent installed required):

- target table: unknown target names the known ones; Codex rejects `local` and
  forces the user MCP scope; Copilot is project-only and uses the `servers`
  key; JSON paths follow the scope; table paths render with native separators;
  the current server name is not itself a legacy name;
- invocation construction per CLI and transport: Claude's `--` separator,
  Codex's `--url`, Gemini's positional command with no separator, and which
  CLIs carry `-s` on remove;
- probe parsing: Claude and Codex entries in both transports, both HTTP key
  spellings (`url` and `httpUrl`), project keys spelled with either path
  separator;
- process handling: a missing binary reports `AI_AGENT_CLI_MISSING`, not a
  generic failure; Windows resolves shims that are not `.exe`;
- JSON merge: fresh document, foreign servers and unrelated keys preserved, an
  existing entry replaced outright, removal reporting absence, an unparsable
  file refused and left byte-identical;
- rules block: insert into a missing file, insert below user content, in-place
  replace, removal, block-only file removal, malformed and lone-marker
  rejection;
- managed classification: ready, stopped, not installed, drift and scheduler
  error; a non-default port survives into the endpoint; every state is refused
  off Windows;
- transport menu: managed leads when usable, carries its state in the label,
  and is absent when unavailable or broken;
- interactivity: a bare install carries no decision flags, every decision flag
  disables prompting, `--json` and `--quiet` disable it too, and an explicit
  `--transport stdio` is distinguished from the default value.

Integration tests drive the real binary in a temporary project with `HOME` and
`USERPROFILE` redirected, so no test can read or write a real agent
configuration:

- rules-only install creates the block, is idempotent, and preserves
  user-authored content;
- uninstall removes the block and deletes a file AIHelper created, then reports
  `not present`;
- the file registrar end to end for Cursor: merge preserving a foreign server
  and an unrelated key, `unchanged` on re-run, clean removal; the nested
  `.cursor/rules/` path is created;
- Copilot writes `servers` in `.vscode/mcp.json` and rejects the user scope;
- `--transport http --url` writes a `url` entry;
- a legacy `ah` registration is removed by both install and uninstall, and
  reported by `ah ai status`;
- `--dry-run` writes no file and creates no configuration;
- an unparsable configuration fails and stays byte-identical;
- argument validation: unknown target, unsupported scope, non-loopback URL,
  `--url` without `--transport http`, both component flags at once;
- `ah ai info --json` documents `ai.install`, `ai.uninstall`, and `ai.status`.

Not covered by automated tests: driving the dialoguer prompts themselves, and
mutating a real agent through its CLI. Both were verified by hand — the latter
against a throwaway `CODEX_HOME` and a temporary project directory, never the
developer's own configuration.

## Documentation

- `docs/reference/ai.md`: new `install`, `uninstall`, `status` sections;
- `docs/reference/mcp.md`: cross-reference from the managed-service section;
- `docs/agents/`: recipe for wiring a fresh agent;
- `src/ai.rs` `host_command_docs()`: add `ai.install`, `ai.uninstall`,
  `ai.status` so the entries appear in `ah ai info`;
- `CHANGELOG.md`.
