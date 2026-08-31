# AIHelper

**Give AI agents safe, predictable tools for real developer work.**

[![CI](https://github.com/Bobsans/AIHelper/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Bobsans/AIHelper/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/Bobsans/AIHelper?sort=semver)](https://github.com/Bobsans/AIHelper/releases/latest)
[![Rust 2024](https://img.shields.io/badge/Rust-2024-orange?logo=rust)](https://www.rust-lang.org/)
[![MCP](https://img.shields.io/badge/MCP-stdio%20%7C%20HTTP-5a67d8)](docs/reference/mcp.md)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-blue)](#platform-support)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

AIHelper is one fast Rust binary that turns common developer workflows into
narrow CLI commands and typed [Model Context Protocol](https://modelcontextprotocol.io/)
tools. Agents get explicit schemas, risk metadata, bounded execution, and compact
results. Developers get the same operations as deterministic text or JSON
without rebuilding shell glue for every repository.

```text
AI agent / MCP client  ─┐
                        ├─>  ah  ─> built-in and dynamic plugins
Developer / automation ─┘
```

[Quick start](#quick-start) · [MCP setup](#connect-an-mcp-client) ·
[Capabilities](#capabilities) · [Safety](#designed-for-agent-safety) ·
[Documentation](#documentation)

## Why AIHelper?

AI agents routinely need to inspect repositories, search files, call APIs,
check Git state, run tools, or query a database. Raw shell access can do all of
that, but every agent must rediscover command syntax, parse unbounded output,
and guess what an operation might change.

AIHelper provides a shared, inspectable command surface instead:

- **Agent-native:** every enabled command can be exposed as a separately typed
  `ah.*` MCP tool.
- **Automation-ready:** stable JSON, deterministic field names, bounded output,
  and explicit exit failures.
- **Risk-aware:** tools publish impact, reversibility, and safety annotations
  before an agent calls them.
- **Context-efficient:** focused file, search, symbol, Git, and project commands
  return only the information requested.
- **Extensible:** built-in domains and ABI-versioned dynamic plugins use the
  same CLI and MCP dispatch path.
- **Local by default:** stdio and HTTP MCP transports run on your machine; no
  hosted AIHelper service is required.

## Quick start

### 1. Install a release binary

Download the archive for your platform from the
[latest GitHub Release](https://github.com/Bobsans/AIHelper/releases/latest):

| Platform | Release asset |
| --- | --- |
| Windows x64 | `ah-windows-x64.zip` |
| Linux x64 | `ah-linux-x64.zip` |
| macOS Apple silicon | `ah-macos-arm64.zip` |

Extract the entire archive and keep the `plugins/` directory next to `ah` or
`ah.exe`. Add that directory to `PATH`, or run the binary from the extracted
folder.

```bash
ah --version
ah --help
ah plugins list
```

### 2. Try the agent-friendly commands

```bash
# Discover the exact commands available in this build.
ah ai info
ah ai info --domain git

# Get a deterministic repository snapshot for automation.
ah --json git status

# Search with bounded context and machine-readable output.
ah --json --limit 100 search text "TODO" src --context 2

# Build a compact code-and-symbol digest for an agent prompt.
ah ctx pack src docs --preset review
```

### 3. Expose the same commands over MCP

```bash
ah --cwd /absolute/path/to/project mcp serve
```

The client now sees each enabled command as a typed tool such as
`ah.file.read`, `ah.search.text`, `ah.git.status`, and `ah.run.check`.

## Connect an MCP client

### Stdio: one process per client

Use `ah` as the MCP server command and pass the workspace explicitly:

```json
{
  "mcpServers": {
    "aihelper": {
      "command": "ah",
      "args": ["--cwd", "/absolute/path/to/project", "mcp", "serve"]
    }
  }
}
```

Do not add `--json` to `mcp serve`; stdout is reserved for MCP protocol
messages. See the [stdio recipe](docs/agents/recipes/mcp-stdio.md) for execution
limits, jobs, cancellation, and lifecycle behavior.

### HTTP: one local server for multiple clients

```bash
ah mcp serve --transport http --port 8787 --max-active 32
```

Connect clients to `http://127.0.0.1:8787/mcp`. AIHelper also exposes a
readiness endpoint at `http://127.0.0.1:8787/health/ready` and identity-aware
local shutdown. The listener is loopback-only and validates Host and Origin,
but it has no authentication; local processes running as the same user should
be treated as trusted.

On Windows, AIHelper can register the HTTP server for the current user and keep
it running through Task Scheduler:

```powershell
ah mcp service status --json
ah --cwd C:\work\project mcp service install
```

The [HTTP and managed-service recipe](docs/agents/recipes/mcp-http.md) includes
ready-to-copy configuration for Claude Code, Codex, and OpenCode.

## Capabilities

AIHelper groups operations by workflow so agents can discover the smallest tool
for a task.

| Workflow | Domains and examples |
| --- | --- |
| Files and context | `file read`, `file tree`, `search text`, `search files`, `ctx pack`, `ctx symbols` |
| Repository work | `git status`, `git diff`, `git blame`, `project detect`, `project commands` |
| Checked execution | `run check` with closed stdin, bounded output, timeout, and process-group termination |
| Reusable automation | `task save`, `task list`, and `task run` for repository-local command recipes |
| HTTP workflows | Requests, expectations, replayed curl commands, and YAML/JSON assertion specs |
| Git providers | Dynamic GitHub and GitLab plugins for issues, releases, GitHub Actions, and GitLab pipelines |
| Data and local AI | Dynamic PostgreSQL inspection/administration and Ollama prompt/chat plugins |
| MCP concurrency | Parallel direct calls plus `ah.job.start`, `status`, `result`, and `cancel` tools |

Run `ah ai info` for the authoritative manual generated by the active plugin
registry. Use `ah plugins list --json` to inspect the exact catalog, required
external tools, ABI version, source, and MCP exposure state.

Detailed command references live under [`docs/reference/`](docs/reference/README.md).

## Designed for agent safety

AIHelper reduces ambiguity at the tool boundary; it is not a security sandbox.
Commands such as process execution or remote mutations can still have the full
permissions of the user running `ah`.

The runtime makes those boundaries visible and enforceable by the caller:

- Every MCP tool publishes standard behavior annotations and
  `_meta["dev.aihelper/risk"]` impact metadata.
- Each call can carry its own `cwd`, output `limit`, and `timeout_ms`; concurrent
  calls never change a process-global working directory.
- Large reads, HTTP responses, release metadata, logs, and expanded archives
  use explicit bounds.
- Operational failures return stable diagnostics instead of successful-looking
  prose.
- MCP jobs report cancellation and timeout immediately while separately
  identifying uncooperative work that is still draining.
- Dynamic plugins are ABI-checked and invalid plugins are skipped with a
  diagnostic instead of preventing the core CLI from starting.

Read the [MCP safety and error contract](docs/reference/mcp.md#safety-metadata-and-errors)
before granting broad tool approvals.

## Signed updates and rollback

AIHelper has a built-in, Windows x64 updater for signed GitHub releases:

```powershell
ah upgrade --check
ah upgrade
ah upgrade --rollback
```

Release manifests are signed with Ed25519. AIHelper verifies the embedded trust
anchor, canonical manifest, target, archive hash, size, and managed file set
before activation. Updates use an external helper, durable transaction state,
an offline candidate smoke test, a verified backup, and automatic rollback for
post-mutation failures. A previously ready managed MCP service is restored only
after the active installation is verified.

The updater changes only signed managed files in the extracted installation; it
does not recursively clean the directory or delete user-owned files. See the
[`ah upgrade` reference](docs/reference/upgrade.md) for compatibility rules,
network limits, recovery behavior, and diagnostics.

## Platform support

| Feature | Windows x64 | Linux x64 | macOS arm64 |
| --- | --- | --- | --- |
| CLI and MCP stdio | Yes | Yes | Yes |
| Foreground Streamable HTTP MCP | Yes | Yes | Yes |
| Packaged dynamic plugins | Yes | Yes | Yes |
| Managed per-user MCP service | Yes | No | No |
| Built-in signed update and rollback | Yes | No | No |

Managed lifecycle and update features support Windows 10, Windows Server 2016,
and newer. Linux and macOS users install newer release archives manually.

## Build from source

AIHelper uses the Rust 2024 edition. A stable Rust toolchain is sufficient for
the repository build used by CI.

```bash
git clone https://github.com/Bobsans/AIHelper.git
cd AIHelper
cargo build --release --locked --bin ah
./target/release/ah --help
```

On Windows, run `target\release\ah.exe --help`. Building only the `ah` binary
provides the built-in domains; use the packaged release archives for the
ready-to-run dynamic plugin layout, or follow the
[plugin developer guide](docs/developers/plugins.md).

Before contributing, run the workspace checks:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo build --locked
```

## How it is built

A Rust workspace: `src/` is the binary and its adapters, `crates/` holds the
libraries it is assembled from - the plugin contract and runtime, the command
domains, the MCP surface, the managed service and the updater - and `plugins/`
holds the dynamic GitHub, GitLab, Ollama, and PostgreSQL crates.

Start with the [architecture guide](docs/developers/architecture.md) and
[plugin guide](docs/developers/plugins.md) before changing runtime contracts.

## Documentation

- [AI-agent recipes](docs/agents/README.md)
- [Command reference](docs/reference/README.md)
- [Developer guide](docs/developers/README.md)
- [MCP reference](docs/reference/mcp.md)
- [Plugin architecture](docs/developers/plugins.md)
- [Invocation logging](docs/reference/logging.md)
- [Changelog](CHANGELOG.md)
- [Release notes](https://github.com/Bobsans/AIHelper/releases)
- [Architecture decisions](docs/decisions/README.md)
- [Contributing](CONTRIBUTING.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [MIT License](LICENSE)

## Contributing

Focused bug reports, documentation improvements, and well-tested plugin or core
changes are welcome. Please read the
[contribution guide](CONTRIBUTING.md) and preserve deterministic
text/JSON output, released field names, and plugin ABI compatibility.

If AIHelper saves your agent from another page of shell glue, consider starring
the repository—it helps more developers find the project.
