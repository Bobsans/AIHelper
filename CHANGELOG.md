# Changelog

All notable changes to this project will be documented in this file.

The format is based on Keep a Changelog, and this project adheres to Semantic
Versioning.

## [Unreleased]

### Fixed

- `gitlab.issues` and `gitlab.pipelines` published an output schema that
  declared four of the ten properties they actually serialize, with
  `additionalProperties: false`, so every typed call returned
  `OUTPUT_SCHEMA_VIOLATION`. Both schemas are now derived from the types that
  produce them and cannot disagree again.
- MCP clients could receive a credential id in an error `cause`. The MCP
  fallback arm rendered the raw `Display` of `SecretNotFound`,
  `SecretKindMismatch`, `VaultLocked` and `VaultKeyUnavailable`; redaction had
  only ever been applied on the CLI side. Both surfaces now project from one
  table.
- `gitlab.job.trace` and `gitlab.job.warnings` left OSC terminal sequences
  (window titles, `ESC ] … BEL`) in the trace text and in the warning scan run
  over it, so a warning wrapped in one did not match. GitLab now uses the same
  stripper as GitHub, which handles both OSC and CSI.
- `postgres.exec` published its `yes` confirmation flag as optional and relied
  on the handler to refuse; it is now required by the schema. `postgres.describe`
  did not require `object` although the extractor errored without it.

### Changed

- Published JSON Schemas are derived from the Rust types they describe rather
  than hand-written, across every built-in domain, the host commands, and all
  four dynamic plugins. The resulting shape differs in four ways that carry no
  meaning: `required` is alphabetical, a nullable field spells itself
  `type: [T, "null"]` instead of `oneOf: [T, null]`, array arguments advertise
  `"default": []`, and an empty `required` is omitted rather than published as
  `[]`. Property names, types and constraints are unchanged.
- `file.stat.kind` and `plugins.list.source`/`state` were documented in prose
  only; they now publish their `enum` values. The three `plugins.*` mutations
  publish `const` on `command`.
- Ollama's decode failure said `failed to decode response from '<url>'` and now
  says `failed to decode ollama response for '<path>'`, matching the GitHub and
  GitLab wording. `OLLAMA_RESPONSE_INVALID` is unchanged.
- A write to stdout that the stream refuses is now reported as
  `OUTPUT_WRITE_FAILED` instead of panicking the process.

### Removed

- `AH_POSTGRES_TEST_SYSTEM_PATH` no longer overrides `psql` resolution. It was
  named as a test seam but shipped in the plugin, ahead of `PATH` itself, and no
  test referenced it.

## [1.4.0] - 2026-08-25

### Added

- `ah ai install`, `ah ai uninstall`, and `ah ai status` wire AIHelper into an
  AI coding agent in one step: they register the `aihelper` MCP server and
  install a managed rules block that points at `ah ai info --json`. Targets are
  `claude`, `codex`, `gemini`, `cursor`, and `copilot`, with `--scope`,
  `--transport <stdio|http|managed>`, `--url`, `--mcp-only`, `--rules-only`,
  `--yes`, and `--dry-run`. Run on a terminal without flags, install asks for
  the scope, the components, and the transport, then confirms a summary before
  changing anything; any flag, a pipe, `--json`, or `--quiet` takes the
  documented defaults instead.
- Registration is delegated to `claude mcp add`, `codex mcp add`, and
  `gemini mcp add`, so AIHelper never rewrites an agent configuration that may
  hold unrelated credentials. Cursor and Copilot have no such CLI, so their JSON
  configurations are merged in place, preserving every other server and key.
  Only loopback MCP endpoints are accepted.
- `ah ai install --transport managed` resolves the endpoint from
  `ah mcp service status` and installs or starts the Windows managed service
  when needed. A drifted registration is reported rather than repaired, and the
  service is never provisioned implicitly: a script must ask for it explicitly.
- `ah ai install` and `ah ai uninstall` also remove a registration left under
  the previous `ah` server name, so an upgrade does not leave an agent holding
  two identical servers.

- `--credential SLOT=ID` now works on the direct CLI for every domain whose
  command catalog declares a secret slot, including `github` and `gitlab`. The
  host resolves the mapping and passes the values in
  `InvocationRequest::resolved_secrets`; the plugin binds them by implementing
  the new `BindResolvedSecrets` trait, whose default rejects unknown slots.
- Vault credentials for the GitHub and GitLab plugins. New secret kinds
  `github-token` and `gitlab-token` hold a single `token` field, and every
  `github.*` and `gitlab.*` command declares an optional `token` credential slot,
  so an agent passes `{"credentials": {"token": "work-github"}}` instead of the
  token itself. A vault credential and an inline `--token` are mutually
  exclusive.
- The protected browser setup pages are styled, responsive, and follow the
  system light or dark theme. A successful browser submission now renders a
  confirmation page with the saved redacted metadata and a Close button instead
  of raw JSON; callers that do not accept `text/html` keep the JSON response.
  Both pages carry `Content-Security-Policy: default-src 'none'` with a
  per-response nonce for their inline style and script.

### Changed

- The CLI and MCP credential paths were unified. The unreleased
  `ah_plugin_argv_to_typed_json_v1` sidecar ABI, `CliTypedInvocation`,
  `CliTypedConversion`, and the per-plugin CLI-to-typed converters are gone; a
  credentialed CLI invocation now runs the plugin's normal argv path instead of
  being rerouted through the typed executor. Dynamic plugins must implement
  `BindResolvedSecrets` for their parsed CLI model.
- MCP rejects an inline `token` argument for `github.*` and `gitlab.*` with
  `INVALID_ARGUMENT`, matching the existing rule for inline HTTP credentials.
  Callers must use the `token` credential slot. The `--token` flag and the
  `GITHUB_TOKEN`, `GH_TOKEN`, `GITLAB_TOKEN`, and `GL_TOKEN` environment
  variables are unaffected for direct CLI use.
- GitHub and GitLab ambient credentials are bound to the destination host.
  `GITHUB_TOKEN`, `GH_TOKEN`, `GITLAB_TOKEN`, `GL_TOKEN`, and the Git credential
  helper now reach only the default API host, the detected git remote host, or a
  loopback host. Any other `--api-url` needs an explicit `--token`.
- The Git credential helper is queried for the host named by `--api-url` instead
  of a fixed host. GitLab additionally requires `--api-url` and `--graphql-url`
  to share one https host before querying the helper.

### Fixed

- Saving a secret from the protected browser setup form failed with
  `LOCAL_REQUEST_REJECTED`. The form was served with `Referrer-Policy:
  no-referrer`, under which browsers send `Origin: null` on the form POST, which
  the local HTTP policy rejects. The form now uses `same-origin`.
- GitHub and GitLab refuse to send any token to a cleartext `http://` API URL
  outside loopback, failing with `GITHUB_INSECURE_TOKEN_TARGET` or
  `GITLAB_INSECURE_TOKEN_TARGET`. GitLab also withholds the token from a
  `--graphql-url` that resolves to another host.
- `--credential` is recognized only before the `--` separator, so a literal
  value after `--` reaches the plugin unchanged.
- Secret values that begin with `-` are redacted from command logs instead of
  being recorded verbatim.
- `AH_VAULT_MASTER_KEY` rejects non-hexadecimal characters instead of accepting
  signed pairs such as `+f`.
- The Git credential helper child process is always reaped, and its output is
  drained while AIHelper waits, so a slow or chatty helper cannot leave a stray
  process or stall on a full pipe.
- `ah ai install` rejects malformed, authenticated, non-HTTP, non-loopback, and
  non-`/mcp` endpoints, and restores the previous CLI-managed registration when
  a replacement fails.
- PostgreSQL `tool.*` commands reject database credentials instead of resolving
  and silently ignoring them.

## [1.3.2] - 2026-08-19

### Fixed

- Windows rollback now finalizes its verified transaction before asking an
  older target binary to reconcile managed MCP, so rollback remains compatible
  with releases that do not allow `mcp service install` through the updater
  recovery fast path.

## [1.3.1] - 2026-08-18

### Fixed

- Windows updater restoration now reconciles the managed MCP definition and
  Task Scheduler registration before starting the service, preventing version
  and restart-policy drift after activation or rollback.
- Recovery launches a verified private helper copy outside the transaction
  directory so Windows can remove completed transaction state without the
  helper locking its own executable.

## [1.3.0] - 2026-08-18

### Added

- `ah http request`, method shortcuts, `replay`, `assert`, and `run` add opt-in
  fixed-delay retries through CLI and typed MCP for transport failures,
  timeouts, response read failures, and HTTP `5xx` responses; client errors and
  assertion failures are not retried.
- HTTP assertion specs can atomically extract JSON values, response headers,
  and regular-expression captures for later case interpolation without exposing
  extracted values in text, JSON, or JUnit reports.

### Changed

- Interactive CLI errors now show human-readable messages, likely command
  corrections with descriptions, valid usage, scoped help commands, and
  operational recovery hints while preserving the released JSON error schema.

### Fixed

- Windows update activation, recovery, and rollback once again accept the
  inherited named-mutex lifecycle lease used by the windowless MCP service.
- The managed MCP launcher now owns its bounded one-minute retries, while stop
  tolerates a Task Scheduler instance disappearing during state readback.

## [1.2.1] - 2026-08-11

### Changed

- Completed `run.check` invocation logs include an optional child outcome with
  only `success`, `timed_out`, and `exit_code`; outer command status and the
  released CLI, JSON/MCP, and plugin C ABI contracts remain unchanged.

### Fixed

- Windows managed MCP launches use a thin windowless service launcher,
  eliminating transient console flashes and the duplicate full-size binary
  while preserving Task Scheduler process ownership.
- Managed MCP status preserves scheduler failures while still reporting trusted
  runtime and readiness evidence, and Task Scheduler readback uses the exact
  rooted task identity.
- `ctx pack` and `ctx symbols` skip files whose complete contents are not valid
  UTF-8 even when the invalid byte appears after the initial binary sniff.

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

[Unreleased]: https://github.com/Bobsans/AIHelper/compare/v1.3.2...HEAD
[1.3.2]: https://github.com/Bobsans/AIHelper/compare/v1.3.1...v1.3.2
[1.3.1]: https://github.com/Bobsans/AIHelper/compare/v1.3.0...v1.3.1
[1.3.0]: https://github.com/Bobsans/AIHelper/compare/v1.2.1...v1.3.0
[1.2.1]: https://github.com/Bobsans/AIHelper/compare/v1.2.0...v1.2.1
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
