# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

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
