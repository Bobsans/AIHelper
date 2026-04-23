# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic Versioning.

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
