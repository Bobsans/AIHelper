# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

## [Unreleased]

## [0.6.3] - 2026-07-13

### Added
- Configurable response and output limits for HTTP bodies, GitHub workflow logs, GitLab job traces, and saved task execution.
- GitLab `--graphql-url` routing and `issue view --full` aggregation for issue details, comments, and designs.

### Changed
- `run check` and `task run` now bound output while reading and terminate descendant process trees on timeout.
- Search traversal is deterministic and ignore-aware, independent of whether `rg` is installed.
- Git and context change detection now parses NUL-delimited porcelain output, preserving unusual paths and rename metadata.
- GitHub and GitLab log processing streams bounded content; issue pagination and pipeline deadline handling are more robust.
- Plugin metadata, manual, invocation, and command boundaries now fail deterministically without changing the plugin ABI.

### Fixed
- Relative `--cwd` handling is applied once, and `run check` preserves child arguments that resemble host-global flags.
- Valid UTF-8 files are no longer classified as binary when the sniff buffer ends inside a multibyte character.

## [0.6.2] - 2026-05-22

### Added
- Dynamic `ah postgres` plugin for PostgreSQL toolchain management, inspection, SQL execution, and diagnostics workflows.
- PostgreSQL command reference documentation and release archive packaging for the Postgres plugin.

### Changed
- Split built-in command domains into smaller domain/io/output modules while preserving their CLI behavior.
- Release workflow now builds and packages `ah-plugin-postgres`.

### Fixed
- `cargo run help` can discover Cargo-built dynamic plugins from profile directories when plugin artifacts are present.
- CI formatting and help integration checks are stable on fresh Linux runners without requiring dynamic plugin artifacts.
- Removed Linux-only warning noise from the Windows command resolver path.

## [0.6.1] - 2026-05-18

### Fixed
- `ah run check` now resolves extensionless Windows commands through `PATHEXT` before spawning them.

## [0.6.0] - 2026-05-12

### Added
- `ah search text` and `ah search files` now accept multiple paths.
- JSON search output now includes `roots` while preserving the existing `root` field.

### Changed
- CLI errors now render in a shorter `CODE: detail` format with concise hints.
- Removed `roadmap.md` and moved stable project intent into the README.

### Fixed
- `ah search text --json` now reports character columns correctly for Unicode text.
- The workspace now passes `cargo clippy --workspace --all-targets --locked -- -D warnings`.

## [0.5.0] - 2026-05-07

### Added
- `ah git commit-info` for commit metadata, touched files, and line stats.
- `ah git tag create` for simple local tag creation.
- `ah project version` for version detection from common manifest files.
- Expanded `ah project detect` with richer snapshot fields for tools, roles, grouped files, versions, and suggested commands.
- Broader `ah project` ecosystem detection for additional languages, platforms, infrastructure, quality, and security tooling.
- Package-manager-aware `ah project commands` suggestions for Node projects plus additional language and infrastructure tools.
- Expanded `ah ctx symbols` heuristics across common programming, infrastructure, config, and script files.

### Changed
- Moved `ah ctx` symbol extraction into a dedicated internal module.

## [0.4.0] - 2026-05-07

### Added
- Dynamic `ah github` plugin for GitHub repository, release, workflow, run, log warning, and artifact inspection.
- Dynamic `ah gitlab` plugin for GitLab project, release, pipeline, job trace, and warning inspection with custom host support.
- Issue list, get, create, update, close, comment, and comment-list commands for `ah github` and `ah gitlab`.
- `ah git status`, `ah git tags`, and `ah git remotes` for compact repository release context.
- Built-in `ah project` domain with `detect` and `commands` helpers.
- Built-in `ah run check` for direct command execution with timeout and bounded output.

### Changed
- CI now tests the full workspace with a locked dependency graph.
- Release archives now package `ah-plugin-github` alongside `ah-plugin-ollama`.
- Release archives now package `ah-plugin-gitlab`.

## [0.3.0] - 2026-05-06

### Added
- Built-in `ah http` domain for HTTP request and API assertion workflows.
- `ah http request` plus method shortcuts for `get`, `post`, `put`, `patch`, and `delete`.
- `ah http replay` for replaying supported curl commands through the stable CLI contract.
- `ah http assert` and `ah http run` for repeatable API checks from spec files, including text, JSON, and JUnit reports.

### Documentation
- Added `docs/reference/http.md` and linked the HTTP domain from the reference index.

### Tests
- Added integration coverage for HTTP request handling, curl replay, assertion specs, reports, and help/manual visibility.

## [0.2.0] - 2026-04-23

### Added
- `ah ai info` command with machine-readable and text manuals aggregated from host commands and plugin-provided metadata.
- Optional plugin manual ABI symbol (`ah_plugin_manual_json_v1`) and manual schema support in `ah-plugin-api`/runtime.
- External dynamic plugin source `plugins/ah-plugin-ollama` with commands:
  - `ah ollama ask` (`/api/generate`)
  - `ah ollama chat` (`/api/chat`)
- Dynamic top-level CLI command registry: plugin domains now appear in `ah help`.
- Release archives now include runtime plugin layout:
  - `ah` / `ah.exe`
  - `plugins/ah-plugin-<name>.<ext>`

### Changed
- Dynamic plugin discovery path moved to `plugins` directory next to executable (`<exe-dir>/plugins`).
- Release workflow now builds both `ah` and `ah-plugin-ollama` and packages them together.
- Top-level command parsing switched to runtime `clap::Command` construction based on loaded plugins.

### Removed
- Local publish script `scripts/publish-release.ps1`.
- Legacy `.release` output convention.

### Documentation
- Added `docs/reference/ollama.md`.
- Updated architecture/plugin/reference docs for executable-relative plugin layout and release artifact structure.

### Tests
- Added/updated tests for:
  - dynamic help domain visibility
  - executable-relative plugin directory resolution
  - manual example parsing in external plugin
  - startup resilience with invalid plugins in runtime plugin directory

## [0.1.0] - 2026-04-23

### Added
- Plugin-oriented runtime architecture with built-in domain plugins (`file`, `search`, `ctx`, `git`, `task`).
- Dynamic plugin loading support from `.ah/plugins` with ABI contract in `ah-plugin-api`.
- `ah plugins list` command for runtime plugin introspection.
- Edge-case safety policy for text operations:
  - binary/non-UTF8 detection
  - large file guard (`--max-bytes`)
  - symlink traversal policy (`--follow-symlinks`)
- New safety-oriented flags across commands:
  - `file read/head/tail --max-bytes --follow-symlinks`
  - `file tree --follow-symlinks`
  - `search text --max-bytes --follow-symlinks`
  - `search files --follow-symlinks`
  - `ctx pack/symbols --max-bytes --follow-symlinks`
- Skip metrics in JSON output for `search text` and `ctx pack/symbols`:
  - `skipped_binary_files`
  - `skipped_large_files`
  - `skipped_symlink_files`
- Release tooling:
  - `scripts/publish-release.ps1` for clean local publish output to `.release/ah.exe`
  - GitHub Release workflow with multi-platform binaries (Windows, Linux, best-effort macOS) packaged as `ah-<platform>-<arch>.zip`.

### Changed
- Command help output now includes subcommand descriptions for plugin domains.
- Runtime startup hardening:
  - invalid dynamic plugins are skipped instead of aborting startup
  - warnings are emitted for skipped plugins (unless `--quiet`)
- Dynamic plugin response handling now always frees returned C strings, including error paths.

### Documentation
- Expanded plugin development and reference documentation.
- Updated command reference docs for new safety flags and behavior.

### Tests
- Added runtime and integration coverage for plugin loader resilience and edge-case handling.
- Smoke test suite expanded and stabilized for new safety and plugin behaviors.
