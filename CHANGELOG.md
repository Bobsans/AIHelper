# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic
Versioning.

## [Unreleased]

### Fixed

- Windows managed MCP launches use a thin windowless service launcher,
  eliminating transient console flashes and the duplicate full-size binary
  while preserving Task Scheduler process ownership.

## [1.2.0] - 2026-08-06

### Added

- Local Streamable HTTP MCP serving adds exact readiness identity,
  identity-aware control shutdown, and the transport-independent
  `ah.job.start`, `ah.job.status`, `ah.job.result`, and `ah.job.cancel` tools
  over bounded fail-fast parallel execution.
- `ah mcp service install`, `start`, `stop`, `restart`, `uninstall`, and `status`
  manage a per-user Windows Task Scheduler service with exact readiness,
  deterministic JSON status, idempotent lifecycle operations, and bounded
  restart policy.
- Windows x64 installations gain signed `ah upgrade --check`, `ah upgrade`,
  exact `ah upgrade --version VERSION`, and offline
  `ah upgrade --rollback`, including durable interruption recovery, one verified
  permanent backup, and managed MCP restoration.
- Release publication produces canonical Ed25519 manifest and detached-signature
  companions for every platform archive; the Windows archive also includes the
  isolated `ah-update-helper.exe` activation and recovery binary.

### Changed

- Standalone `ah --version` and `ah -V` requests bypass runtime startup and
  invocation logging for lower latency.
- Windows native command execution now assigns processes to a Job Object during
  creation, avoiding the previous system-wide thread scan while preserving
  timeout and cancellation of descendant processes. The minimum supported
  Windows releases are Windows 10 and Windows Server 2016.
- MCP invocation logs now separate queue wait from execution time and identify
  queue versus execution timeout without changing the existing total duration.
- Release archives are smoke-tested through CLI plugin discovery and MCP tool
  listing on every packaged platform before upload.
- `git status` now collects branch and worktree state from one porcelain v2
  snapshot and reads commit/tag metadata concurrently, reducing child process
  startup overhead without changing output fields.

### Fixed

- MCP readiness, control shutdown, active-job cancellation, and managed-service
  recovery now preserve exact process identity and report bounded shutdown or
  restart failures instead of accepting ambiguous state.
- Update activation, recovery, and rollback preserve durable state when helper
  termination is uncertain, retain operation-specific diagnostics, and bound
  helper subprocess time, output, and descendant lifetime.

### Security

- The updater verifies canonical manifests, detached Ed25519 signatures, archive
  identity, and every signed managed-file digest against an embedded production
  trust anchor before activation; private signing material remains outside the
  repository and runtime binaries.
- Release signing rejects unsafe archive entry types and unsupported compression,
  bounds state reads during I/O, and pins external GitHub Actions to immutable
  commit SHAs.

## [1.1.0] - 2026-07-20

### Added

- Best-effort daily JSONL logging for completed CLI and MCP invocations, with
  structured error diagnostics, ten-day retention, bounded records, and
  concurrent-process locking.

### Security

- Invocation logs redact secret-bearing values by default;
  `AH_LOG_UNREDACTED=1` provides an explicit opt-in for isolated diagnostics
  while preserving record-size limits.

## [1.0.0] - 2026-07-17

### Added

- `ah mcp serve` exposes every enabled typed command as a separate MCP tool over
  stdio, with input/output schemas, structured results, cancellation, and live
  tool-list updates.
- Transport-neutral typed command contracts and dynamic plugin sidecar symbols
  support catalog discovery, invocation, and cancellation.
- Built-in, host, GitHub, GitLab, Ollama, and PostgreSQL tools expose risk,
  impact, effects, and reversibility metadata.

### Changed

- Runtime execution now uses a bounded sequential FIFO abstraction with
  request-scoped cwd, limits, deadlines, and cancellation, ready for a future
  resource-aware parallel scheduler.
- Cargo profile plugin artifacts take precedence over older executable-adjacent
  development copies while packaged plugin discovery remains unchanged.

## [0.6.3] - 2026-07-13

### Added

- Configurable response and output limits cover HTTP bodies, GitHub workflow
  logs, GitLab job traces, and saved task execution.
- GitLab supports `--graphql-url` routing and `issue view --full` aggregation for
  issue details, comments, and designs.

### Changed

- `run check` and `task run` now bound output while reading and terminate
  descendant process trees on timeout.
- Search traversal is deterministic and ignore-aware, independent of whether
  `rg` is installed.
- Git and context change detection now parses NUL-delimited porcelain output,
  preserving unusual paths and rename metadata.
- GitHub and GitLab log processing streams bounded content; issue pagination and
  pipeline deadline handling are more robust.
- Plugin metadata, manual, invocation, and command boundaries now fail
  deterministically without changing the plugin ABI.

### Fixed

- Relative `--cwd` handling is applied once, and `run check` preserves child
  arguments that resemble host-global flags.
- Valid UTF-8 files are no longer classified as binary when the sniff buffer
  ends inside a multibyte character.

## [0.6.2] - 2026-05-22

### Added

- The dynamic `ah postgres` plugin provides PostgreSQL toolchain management,
  inspection, SQL execution, and diagnostic workflows with command reference
  documentation.
- Release archives include `ah-plugin-postgres` alongside the existing runtime
  plugins.

### Changed

- Built-in command domains are split into smaller domain, I/O, and output
  modules while preserving their CLI behavior.

### Fixed

- `cargo run help` discovers Cargo-built dynamic plugins from profile
  directories when plugin artifacts are present.
- CI formatting and help integration checks are stable on fresh Linux runners
  without requiring dynamic plugin artifacts.
- Windows command resolution no longer emits Linux-only warning noise.

## [0.6.1] - 2026-05-18

### Fixed

- `ah run check` resolves extensionless Windows commands through `PATHEXT`
  before spawning them.

## [0.6.0] - 2026-05-12

### Added

- `ah search text` and `ah search files` accept multiple paths.
- JSON search output includes `roots` while preserving the existing `root`
  field.

### Changed

- CLI errors render in a shorter `CODE: detail` format with concise hints.
- Stable project intent moved from the removed `roadmap.md` file into the
  README.

### Fixed

- `ah search text --json` reports character columns correctly for Unicode text.
- The workspace passes
  `cargo clippy --workspace --all-targets --locked -- -D warnings`.

## [0.5.0] - 2026-05-07

### Added

- `ah git commit-info` reports commit metadata, touched files, and line stats.
- `ah git tag create` creates simple local tags.
- `ah project version` detects versions from common manifest files.
- `ah project detect` returns richer snapshots with tools, roles, grouped files,
  versions, and suggested commands.
- Project detection covers additional languages, platforms, infrastructure,
  quality, and security tooling.
- `ah project commands` provides package-manager-aware suggestions for Node
  projects plus additional language and infrastructure tools.
- `ah ctx symbols` recognizes more common programming, infrastructure, config,
  and script files.

### Changed

- `ah ctx` symbol extraction moved into a dedicated internal module without
  changing its command contract.

## [0.4.0] - 2026-05-07

### Added

- The dynamic `ah github` plugin supports repository, release, workflow, run,
  log warning, and artifact inspection.
- The dynamic `ah gitlab` plugin supports project, release, pipeline, job trace,
  and warning inspection with custom host support.
- `ah github` and `ah gitlab` support issue list, get, create, update, close,
  comment, and comment-list workflows.
- `ah git status`, `ah git tags`, and `ah git remotes` provide compact repository
  release context.
- The built-in `ah project` domain provides `detect` and `commands` helpers.
- The built-in `ah run check` command executes direct checks with timeout and
  bounded output.

### Changed

- CI tests the full workspace with a locked dependency graph.
- Release archives package `ah-plugin-github` and `ah-plugin-gitlab` alongside
  `ah-plugin-ollama`.

## [0.3.0] - 2026-05-06

### Added

- The built-in `ah http` domain supports HTTP request and API assertion
  workflows.
- `ah http request` includes method shortcuts for `get`, `post`, `put`, `patch`,
  and `delete`.
- `ah http replay` replays supported curl commands through the stable CLI
  contract.
- `ah http assert` and `ah http run` provide repeatable API checks from spec
  files with text, JSON, and JUnit reports.
- The HTTP command reference and integration coverage document and protect
  request handling, curl replay, assertion specs, reports, and help/manual
  visibility.

## [0.2.0] - 2026-04-23

### Added

- `ah ai info` provides machine-readable and text manuals aggregated from host
  commands and plugin-provided metadata.
- The optional `ah_plugin_manual_json_v1` ABI symbol and manual schema support
  plugin-provided manuals through `ah-plugin-api` and the runtime.
- The external `ah-plugin-ollama` plugin provides documented `ah ollama ask`
  (`/api/generate`) and `ah ollama chat` (`/api/chat`) commands.
- The dynamic top-level CLI registry shows plugin domains in `ah help` and
  validates plugin-provided manual examples.
- Release archives use an executable-relative runtime layout containing `ah` or
  `ah.exe` plus `plugins/ah-plugin-<name>.<ext>`; the architecture, plugin, and
  reference documentation describe the same layout.

### Changed

- Dynamic plugin discovery moved to the executable-relative `plugins`
  directory, with integration coverage for discovery and startup resilience in
  the presence of invalid plugins.
- The release workflow builds and packages both `ah` and `ah-plugin-ollama`.
- Top-level command parsing uses runtime `clap::Command` construction based on
  loaded plugins.

### Removed

- The local `scripts/publish-release.ps1` publishing workflow was removed.
- The legacy `.release` output convention was removed.

## [0.1.0] - 2026-04-23

### Added

- A plugin-oriented runtime architecture provides built-in `file`, `search`,
  `ctx`, `git`, and `task` domain plugins, with accompanying plugin development
  and command reference documentation.
- Dynamic plugins load from `.ah/plugins` through the ABI contract in
  `ah-plugin-api`.
- `ah plugins list` provides runtime plugin introspection.
- Text operations detect binary and non-UTF-8 input, guard large files with
  `--max-bytes`, and require `--follow-symlinks` for symlink traversal.
- Safety flags cover `file read/head/tail`, `file tree`, `search text`,
  `search files`, and `ctx pack/symbols`.
- JSON output for `search text` and `ctx pack/symbols` reports
  `skipped_binary_files`, `skipped_large_files`, and `skipped_symlink_files`.
- Release tooling provides a clean local `.release/ah.exe` build through
  `scripts/publish-release.ps1` and multi-platform GitHub archives named
  `ah-<platform>-<arch>.zip`.

### Changed

- Command help includes subcommand descriptions for plugin domains.
- Invalid dynamic plugins are skipped instead of aborting startup, with warnings
  emitted unless `--quiet`; runtime and integration coverage protect this
  behavior.
- Dynamic plugin responses always free returned C strings, including error
  paths.
- Runtime and integration smoke coverage protects plugin loading, edge-case text
  handling, and safety behavior.

[Unreleased]: https://github.com/Bobsans/AIHelper/compare/v1.2.0...HEAD
[1.2.0]: https://github.com/Bobsans/AIHelper/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/Bobsans/AIHelper/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/Bobsans/AIHelper/compare/v0.6.3...v1.0.0
[0.6.3]: https://github.com/Bobsans/AIHelper/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/Bobsans/AIHelper/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/Bobsans/AIHelper/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/Bobsans/AIHelper/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/Bobsans/AIHelper/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/Bobsans/AIHelper/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/Bobsans/AIHelper/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/Bobsans/AIHelper/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/Bobsans/AIHelper/tree/v0.1.0
