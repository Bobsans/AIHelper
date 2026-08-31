# Architecture Overview

AIHelper is a Rust workspace built around a plugin contract: 23 library crates,
four dynamic plugin crates, and a root binary that wires them together. Command
implementations do not know which transport invoked them, and the transports do
not know which plugin answers.

## Workspace Layout

### Root crate (`aihelper`)

The binary, and nothing a library could own. Every module here is either a
process concern or an adapter between the CLI and a crate below.

| Path | Owns |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | process entrypoint, runtime bootstrap |
| `src/entry.rs` | the static pre-parse that answers `upgrade` and `mcp service` before plugin discovery runs |
| `src/cli/` | the clap tree (`command`), one parse into one `RuntimeCommand` (`parse`), the `run check` passthrough walker, argv redaction, and the did-you-mean suggester |
| `src/runtime_flow/` | one run in four phases: parse, bootstrap, dispatch, report |
| `src/plugins/` | the eight built-in plugin adapters, one module per domain |
| `src/host_commands.rs` | host-only typed commands (`ai.*`, `plugins.*`) that no plugin provides |
| `src/service/`, `src/upgrade/` | CLI routing and rendering for the managed service and the updater; both mechanisms live in crates and print nothing themselves |
| `src/plugin_settings.rs` | the `plugins.json` enabled-state document |
| `src/harness.rs` | running a command in-process and reading its rendered output, so an assertion about text costs no subprocess |
| `src/snapshots.rs`, `src/reference_docs.rs` | the golden snapshots and the drift checks that keep `docs/` tied to the catalog |
| `src/bin/ah-mcp-service.rs` | the server process the managed service schedules |

### The plugin contract

- `ah-plugin-api` — stable request/response payloads, the C ABI structures
  (`AhPluginApiV1`) and symbol constants, and transport-neutral typed command
  descriptors, effects, execution context and structured errors.
- `ah-plugin-sdk` — shared implementation for what every plugin would otherwise
  rewrite: `credentials` (where a token may be sent, and where an unattended one
  may come from), `http` (a blocking JSON client with authority-bound
  credentials and uniform error mapping), `render` (success rendering, ANSI
  stripping, truncation). A Rust-side convenience only: the C ABI is unchanged,
  so a plugin built without it still loads.
- `ah-runtime` — the plugin manager, the built-in and dynamic registry, the
  dynamic loader for `.dll`/`.so`/`.dylib`, typed schema validation, and bounded
  execution.
- `ah-plugin-testkit` — the HTTP mock server the plugin test suites share, so a
  fix to it reaches all of them.

### The command domains

- `ah-domains` — `file`, `search`, `ctx`, `git`, `project`, `run`, `http`,
  `task` and `secrets`. One module per domain in the same three layers:
  `domain` is pure, `io` performs effects, `output` renders. `layout.rs` fails
  the build when that shape drifts, because prose in a contributing guide had
  already failed to hold the line twice. The domains know nothing of the CLI.
- `ah-ai` — `ah ai install`/`status`/`uninstall`: registering AIHelper as an MCP
  server with Claude Code, Codex, Gemini, Cursor and Copilot, each with its own
  configuration format and location.
- `ah-secrets` — the credential vault: where secrets are kept, and how a
  `--credential SLOT=ID` reference resolves to a value the command never logs.
- `ah-setup-ui` — the browser-facing secret entry and confirmation pages, their
  Content-Security-Policy, per-page nonce, `no-store` and referrer headers, HTML
  escaping and `Accept` negotiation. A web application rather than an MCP
  concern, so its security properties are reviewed on their own.

### Cross-cutting foundations

Each of these exists because more than one caller needed the same answer, and
two callers had already given different ones.

- `ah-error` — the error type, and what a user sees when one reaches the
  surface.
- `ah-output` — the options a command reports under, and the `Emitter` that
  writes them. No adapter calls `println!`.
- `ah-redact` — the single redaction engine: rewriters (`sanitize_*`) for sinks
  that record a value, detectors (`url_contains_userinfo`, `curl_contains_auth`,
  `is_authorization_header`) for callers that must reject one. Consumed by the
  event log, the CLI argv logger and the MCP adapter, so a heuristic added once
  protects every sink.
- `ah-observability` — the event log: what happened, redacted, rotated, and
  readable after the fact.
- `ah-paths` — one answer to where AIHelper's files live, shared by the host,
  the service, the updater and the plugins.
- `ah-config` — where configuration is read from, and what a relative override
  is relative to.
- `ah-persist` — atomic, single-writer JSON state files.
- `ah-platform` — the platform-specific primitives, behind one signature each.

### The MCP surface

`ah-mcp`:

- `server` — the `rmcp` handler, tool dispatch and catalog snapshots
- `transport` — stdio and local stateful Streamable HTTP, readiness, the control
  and setup routes, and the policy deciding a request is local
- `mapping` — catalog commands, typed responses and errors into MCP types
- `job_tools` and `jobs` — the `ah.job.*` surface and the registry behind it
- `shutdown` — the shutdown request, tracker and stdin reader
- `plaintext_auth` — refusing and scrubbing credentials pasted into arguments

### Managed service and self-update

- `ah-service` — the managed MCP service: a definition on disk, a scheduled task
  that runs it, and a server process that answers. Three layers that can each be
  wrong independently, which is why `status` is a reduction rather than a read.
  The scheduler is a port: Windows Task Scheduler and systemd user units are
  adapters behind it, and macOS has none, so `ah mcp service` refuses there.
  Adding `launchd` is writing an adapter and verifying it against a real Mac; the
  plist projection alone has no consumer.
- `ah-updater` — the self-update mechanism: check, download, verify, activate,
  roll back, recover. Renders nothing and reads no command line; `src/upgrade/`
  supplies both.
- `ah-updater-core` — the network- and filesystem-independent policy core:
  version comparison, the update plan, trust anchors, installation shape.
- `ah-update-helper` — the external process that performs the mutation, so the
  binary being replaced is not the one holding the file open.
- `ah-release-manifest` — the strict schema and verification contract for
  release manifests. See
  [Release manifest v1](../decisions/2026-07-22-adopt-canonical-ed25519-release-manifest-v1.md).
- `ah-release-tool` — release-only archive validation and signing for CI.
  Deliberately not a dependency of the shipped `ah` binary.

### Dynamic plugins

`plugins/ah-plugin-github`, `-gitlab`, `-ollama`, `-postgres` build as cdylibs,
load from the `plugins/` directory next to the executable, and reach the same
dispatch path as a built-in domain.

## Runtime Flow

1. CLI parses global options and domain argv.
2. Runtime initializes plugin manager.
3. Built-in plugins are registered
   (`file/search/ctx/git/project/run/http/task`).
4. Dynamic plugins are loaded from `plugins` directory next to `ah` executable (if present).
5. Global plugin-state config is loaded (`plugins.json`) and disabled domains are applied.
6. Domain invocation is routed to plugin:
   - dynamic plugin takes precedence for same domain
   - otherwise built-in plugin handles request
   - disabled domain returns `DOMAIN_DISABLED`
7. Legacy CLI invocation returns `InvocationResponse` (`success/error`).

For `ah mcp serve`, the same bootstrap is followed by:

1. Registering host-only typed commands (`ai.info`, `plugins.*`, `secrets.list`).
2. Validating every enabled typed command and output schema.
3. Starting the fail-fast parallel executor and selected `rmcp` transport.
4. Mapping each descriptor to `ah.<command-id>`.
5. Returning validated structured content or a structured diagnostic.

MCP completion events use a bounded process-local dispatcher backed by one
dedicated thread. Command handlers never wait for logging capacity and never
share Tokio's blocking pool with event sinks. Shutdown closes admission at EOF
or signal time and spends one common five-second budget across transport drain
and runtime termination.

The runtime compiles typed input/output validators into an immutable registry
once per plugin-definition revision. Enabled-state changes have a separate
revision and the MCP adapter swaps complete immutable tool snapshots, so normal
lookup and invocation do not rebuild or serialize the full catalog.

## Plugin Contract

- Host and plugins communicate via `InvocationRequest`/`InvocationResponse` (JSON payload).
- Dynamic plugins expose `ah_plugin_entry_v1` with `AhPluginApiV1`.
- ABI compatibility is validated by runtime (`AH_PLUGIN_ABI_VERSION`).
- Plugin metadata includes:
  - plugin name
  - domain
  - description
  - ABI version

Typed dynamic plugins advertise `typed_commands_v1` and expose the catalog,
invoke, and cancellation sidecar symbols as one complete capability. The
plugin API does not depend on `rmcp`; MCP types remain confined to
`crates/ah-mcp`.

## Command Contracts

- Commands remain domain-scoped. Eight domains are built-in plugins
  (`file`, `search`, `ctx`, `git`, `project`, `run`, `http`, `task`); `ai`,
  `plugins` and `secrets` are host commands; `github`, `gitlab`, `ollama` and
  `postgres` arrive as dynamic plugins.
- Global flags (`--json`, `--quiet`, `--cwd`, `--limit`) are converted to plugin wire options.
- Output contract:
  - text by default
  - JSON when `--json`
  - optional suppression with `--quiet`

The CLI applies `--cwd` once, before plugin discovery and configuration lookup.
Startup argument scanning stops at `--`; for `run check`, the child command and
its remaining arguments are opaque to host-global normalization. This preserves
child flags such as `--json`, `--limit`, and `--cwd` without changing host state.

Typed commands add:

- JSON Schema input and output contracts
- explicit effects, risk, impact, and reversibility metadata
- request-scoped `cwd`, `limit`, deadline, and request id
- structured success, notices, and errors

The typed command kernel is transport-neutral. CLI and MCP adapters share the
same command descriptors and typed handlers; protocol-specific `rmcp` types stay
inside `crates/ah-mcp`. Human-readable CLI rendering remains an adapter concern
rather than part of the structured command result.

Typed execution carries `cwd` explicitly with each request. Handlers resolve
relative paths from that context and set child-process working directories with
`Command::current_dir`; they never mutate the process-wide current directory per
request. The MCP adapter does not impose a path sandbox. Operating-system process
permissions are the filesystem and process access boundary.

The executor admits up to `--max-active` physical handlers with non-blocking
permit acquisition. Accepted handlers overlap; capacity exhaustion fails
immediately and never queues work. Direct MCP calls and `ah.job.start` targets
share the same permits. Logical cancellation or timeout may precede physical
handler exit, so an uncooperative handler retains only its own permit while
draining.

Every admitted call receives a unique internal execution ID. MCP protocol request
IDs map to those execution IDs only for the lifetime of the call, so reused protocol
IDs and late cleanup cannot target a later execution. Cooperative handlers install
request-local cancellation scopes that preserve pre-delivered cancellation, check
it before command work, and remove local and registry state on normal return or
panic.

Plugin settings and task stores use a bounded sidecar lock for cross-process
read-modify-write operations and atomically replace complete JSON documents.
The transaction reloads current state under the lock, writes and syncs a temporary
sibling, then publishes in-memory state only after persistence succeeds. Readers
therefore observe either the old complete document or the new complete document,
and concurrent writers do not lose unrelated updates.

## Deterministic I/O Boundaries

Commands bound data while reading it, rather than after an unbounded read. The
`run` domain drains stdout and stderr concurrently with independent byte budgets;
prefix mode retains the beginning of each stream, while tail mode retains a
bounded suffix. Public strings are converted to lossy UTF-8 only after capture.

Command deadlines cover the complete descendant process tree and inherited
pipes. Unix process groups and Windows Job Objects provide the platform-specific
termination boundary, so descendants cannot outlive a timeout or keep readers
blocked beyond it.

HTTP response bodies are also capped during the read. A truncated response keeps
status and header metadata, but body- and JSON-derived assertions require a
complete body and therefore fail explicitly. Callers can distinguish truncation
from an ordinary response through structured metadata.

Archive-backed provider logs have separate budgets for the compressed response
and cumulative expanded content. Entries are filtered while they are read, and
budget overflow fails explicitly instead of returning a misleading partial
archive result.

Text safety checks that inspect a bounded byte prefix distinguish an incomplete
UTF-8 sequence at the end of that prefix from definite malformed input. A
boundary-only incomplete sequence remains eligible as text; NUL bytes and known
invalid sequences remain binary. Full readers still validate content beyond the
sniff boundary. The `file`, `ctx`, and `search` domains share this policy.

Repository discovery and status parsing do not depend on optional host tools.
Search uses one ignore-aware traversal path with a stable backend identifier.
`git status`, `git changed`, and `ctx changed` share a byte-oriented parser for
NUL-delimited porcelain output; path bytes are converted to public strings only
at the response boundary.

Text search applies the remaining global result budget during each file scan. It
keeps file text once, allocates context only for returned matches, checks
cancellation while scanning lines, and uses one sentinel match beyond the budget
to report truncation exactly. Context-before and context-after arrays are derived
from the requested line window without off-by-one expansion.

## Error Model

- Domain logic returns `AppError` codes.
- Plugin runtime normalizes errors into `InvocationResponse.error_code/error_message`.
- Host converts runtime failures to process-level non-zero exit.
