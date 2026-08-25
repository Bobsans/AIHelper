# 03 — Composition, Bootstrap and Configuration

**Severity: High.** The startup path accumulated special cases faster than it
acquired structure. It is now the least predictable part of the system and the
hardest to test.

## Findings

### 3.1 Seven parsers run before the real parser

[`src/runtime_flow.rs:33`](../../src/runtime_flow.rs) `run()` inspects raw
`Vec<OsString>` argv repeatedly before clap ever sees it:

| Pre-parser | Location | Matches on |
|---|---|---|
| `is_installed_smoke_fast_path` | `runtime_flow.rs:143` | env var + `--version` |
| `is_updater_mcp_restore_fast_path` | `runtime_flow.rs:147` | env var + exact 5-element argv |
| `is_managed_serve_request` | `runtime_flow.rs:157` | `mcp serve` window + `--managed-config` prefix |
| `is_version_fast_path` | `runtime_flow.rs:165` | argv length 2 |
| `updater::command::route` | `src/updater/command.rs` | `upgrade` |
| `mcp_service::command::route` | `src/mcp_service/command.rs` | `mcp service …` |
| `prepare_run_check_passthrough` | `src/cli.rs` | `run check` passthrough split |

`is_updater_mcp_restore_fast_path` is the clearest symptom: it hard-codes
`raw_args.len() == 5` and positional string comparisons. Any flag reordering,
an added `--quiet`, or an `=`-style argument silently disables the path.

### 3.2 Control flow driven by undocumented environment variables

`AH_UPDATER_INSTALLED_SMOKE` and `AH_UPDATER_MCP_RESTORE`
([`src/runtime_flow.rs:143`, `:147`](../../src/runtime_flow.rs)) change what the
process does at startup. They are internal handoff channels between `ah`, the
updater and the update helper, but they are indistinguishable from user
configuration and are not covered by the CLI contract, the docs, or `--help`.

### 3.3 `run()` mixes eight unrelated responsibilities

In order: smoke fast path, crash recovery, version fast path, argv redaction,
process-wide `chdir`, upgrade routing, managed-service routing, logger
construction, runtime startup, plugin discovery, CLI routing, diagnostics
rendering, execution, and outcome logging — one function, ~110 lines, no seam
where a test can enter.

### 3.4 The process working directory is mutated globally

[`src/cli.rs:120`](../../src/cli.rs) `apply_initial_cwd_from_raw_args` calls
`std::env::set_current_dir`. Meanwhile the typed path carries `context.cwd`
per request ([`crates/ah-plugin-api/src/lib.rs:524`](../../crates/ah-plugin-api/src/lib.rs)),
and `GitIo::at(cwd)` / `GitIo::current()`
([`src/commands/git/io.rs:14`](../../src/commands/git/io.rs)) support both.

So there are two competing notions of "where am I", one of which is
process-global. In `mcp serve` the executor runs commands in parallel
([`crates/ah-runtime/src/executor.rs`](../../crates/ah-runtime/src/executor.rs));
any code path that still consults the process cwd is racy by construction. The
MCP adapter compensates with `require_explicit_cwd`
([`crates/ah-mcp/src/server.rs:2244`](../../crates/ah-mcp/src/server.rs)) — a
guard that exists because the underlying model is ambiguous.

### 3.5 The configuration model advertises layering it does not implement

[`src/config.rs:20`](../../src/config.rs) declares a five-level priority list
(`Env`, `Flags`, `Project`, `User`, `Defaults`), but `Flags` and `Project` are
never produced anywhere in the codebase: `config_dir_source` is only ever `Env`
or `User`, and `plugin_dirs_source` is hard-coded to `Defaults`
(`src/config.rs:55`). `ConfigContext` resolves exactly two things (config dir,
plugin dirs) and is reported to users as if it were a layered config system.

Meanwhile actual configuration is scattered: `plugins.json` via
`PluginSettings`, the vault via `secrets::`, service paths via
`mcp_service::paths`, updater state under `APPDATA`, and per-plugin ad-hoc
resolution (`plugins/ah-plugin-postgres/src/lib.rs` reads `AH_CONFIG_DIR`,
`AH_CACHE_DIR`, `XDG_*`, `APPDATA`, `LOCALAPPDATA` itself).

### 3.6 Startup work is unconditional

`EventLogger::new()`, `ConfigContext::load()`, plugin discovery and dynamic
library loading run for every invocation, including `ah file read`. The
existence of hand-written "fast paths" for `--version` is evidence that startup
cost was noticed and worked around rather than structured.

## Why it hurts

- Every new entry mode (a second service, a new helper handoff) adds another
  string-sniffing pre-parser, and the interactions between them are untested.
- Nothing in the startup sequence can be unit-tested; all coverage is via
  `assert_cmd` subprocesses (group 08).
- The cwd ambiguity is a latent concurrency bug in the MCP server, which is the
  flagship execution mode.
- The config abstraction misleads: a contributor reading `CONFIG_SOURCE_PRIORITY`
  will assume project-level config exists.

## Target design

### A. One parse, one command enum

Parse argv exactly once, with clap, into a total enum:

```rust
enum Entry {
    Version,
    Upgrade(UpgradeRequest),
    Service(ServiceCommand),
    ManagedServe { definition: PathBuf },
    Mcp(McpServe),
    Domain { domain: String, argv: Vec<String> },
    HelperHandoff(HandoffKind),   // replaces the env-var fast paths
}
```

Internal handoff modes become hidden subcommands (`clap` `hide = true`), not
environment variables and not positional argv matching. They are then covered by
the same parser, the same tests, and the same redaction.

### B. Explicit bootstrap phases with declared needs

```rust
Entry::needs() -> Needs { config: bool, plugins: bool, logging: bool, vault: bool }
```

`bootstrap(entry.needs())` builds only what the entry point requires. The
`--version` fast path stops being a special case and becomes `Needs::none()`.

### C. Working directory is data, never process state

- Remove `std::env::set_current_dir` entirely.
- `--cwd` populates a field on the request context for both CLI and MCP paths.
- All IO adapters take the cwd explicitly (`GitIo::at` already does; make it the
  only constructor and delete `GitIo::current`).
- `require_explicit_cwd` in the MCP adapter can then be simplified or removed.

### D. Honest configuration

Two options, and the choice should be deliberate:

1. **Implement the layering** — add project-level (`.ah/config.toml` discovered
   upward) and flag-level sources, resolve *all* configuration through one
   `Config` value (log dir, plugin dirs, vault path, cache dir, service paths,
   plugin-specific settings), and expose provenance per key.
2. **Delete the pretence** — drop `Flags`/`Project` from `ConfigSource` and
   document that AIHelper is configured by env var plus `plugins.json`.

Option 1 is the right long-term answer for a tool that already spans user, project
and CI contexts; option 2 must be shipped immediately if option 1 is deferred, so
the code stops describing a system that does not exist.

Either way: plugins should receive resolved paths through the request context
instead of re-deriving `XDG`/`APPDATA` themselves.

## Migration

1. Delete `ConfigSource::Flags`/`Project` (or implement them) — smallest, highest
   clarity-per-line change; do it first.
2. Convert the two env-var fast paths into hidden subcommands; keep the env vars
   as a deprecated alias for one release so an in-flight updater can still hand off.
3. Fold `updater::command::route`, `mcp_service::command::route`,
   `prepare_run_check_passthrough` and the remaining argv sniffers into a single
   clap parse producing `Entry`.
4. Split `run()` into `parse → bootstrap(needs) → dispatch → report`; each stage
   independently testable, `run()` becomes ~20 lines.
5. Remove `set_current_dir`; thread cwd through the request context; make
   `GitIo::at` the only constructor. Add a test that runs two commands
   concurrently in different directories through the executor.
6. Consolidate path resolution into one `ah-paths` module used by host, service,
   updater and plugins.

## Risks and invariants

- **Step 2 touches the updater handoff**, which runs across two binaries and two
  versions. The deprecated alias window is mandatory: a v1.4 helper must still be
  able to hand off to a v1.5 `ah` and vice versa.
- **Removing `set_current_dir` may change relative-path resolution** in commands
  that currently rely on the ambient cwd. Enumerate them (`GitIo::current`,
  `run` program resolution `src/commands/run/io.rs:291`, file domain path joins)
  before the change and cover each with a test.
- **Startup ordering matters**: crash recovery currently runs before argument
  parsing on purpose. Preserve that ordering — recovery must not depend on a
  successful parse.

## Acceptance criteria

- Exactly one call site parses argv.
- No production code branches on an `AH_*` environment variable to decide *what
  command to run*.
- `std::env::set_current_dir` does not appear in the workspace.
- `run()` fits on one screen and each of its stages has direct unit tests.
- `ConfigSource` variants all have producers, or no longer exist.
