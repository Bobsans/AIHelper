# Workspace Contract Tests Design

## Context

AIHelper has black-box integration coverage for built-in CLI domains and unit/mock
coverage in the root crate, `ah-plugin-api`, `ah-runtime`, and four dynamic plugin
crates. Coverage depth is uneven. The `file` domain is the reference implementation:
its public text and JSON contracts, relevant errors, limits, safety behavior, and
cross-platform cases are covered through the real `ah` binary.

This campaign applies the same standard to the entire workspace. It proceeds as
vertical domain slices so each completed increment is independently reviewable and
leaves the workspace green.

## Goals

- Cover every public command with applicable text, JSON, error, boundary, safety,
  and global-option behavior.
- Freeze released JSON field names, types, and ABI behavior without accidental
  breaking changes.
- Exercise built-in domains through the compiled `ah` binary.
- Exercise dynamic plugins with deterministic localhost mocks and fake processes.
- Keep the default suite offline and portable across Windows and Unix.
- Record a durable milestone after each completed module or tightly coupled group.

## Non-goals

- Live requests to GitHub, GitLab, Ollama, PostgreSQL, or download endpoints.
- Tests requiring credentials, a running database, elevated privileges, or fixed
  host ports.
- Literal comparisons of unstable timestamps, platform paths, permissions, or OS
  error wording.
- Snapshot testing, golden-file churn, or a custom scenario DSL.
- Production fixes without a separately confirmed defect and explicit approval.

## Test Architecture

### Built-in domains

Built-in commands remain black-box tests under `tests/integration/` and invoke the
real `ah` binary with `assert_cmd`. JSON is decoded and checked semantically. Text
is compared exactly only when every value is deterministic.

### Core contracts

`ah-plugin-api`, `ah-runtime`, and root infrastructure use focused unit tests for
ABI/version compatibility, request normalization, response ownership, loader
behavior, state, and panic isolation. Only behavior that crosses the CLI boundary
is duplicated in black-box integration tests.

### Dynamic plugins

GitHub, GitLab, and Ollama tests use loopback servers bound to port zero. Tests
assert request method, path, query, headers, and body before validating complete
response contracts. PostgreSQL tests use a cross-platform fake executable when
feasible; they never require a real server.

### Helpers

Helpers start local to the module. A helper moves to shared test infrastructure
only after at least two modules need the same semantics. Helpers may normalize
unstable path separators or newline representation, but must not hide CLI behavior.

## Contract Assertion Policy

- JSON objects: assert the exact set of stable fields, field types, and relevant
  values.
- Text: assert exact output for deterministic data; validate structure and types
  for timestamps and other OS-controlled values.
- Errors: assert stable diagnostic code/message/hint, not nested platform errors.
- Limits: assert returned count, truncation metadata, omitted content, and warning
  behavior in both text and JSON modes where applicable.
- Global options: cover `--json`, `--quiet`, `--limit`, and `--cwd` only where the
  domain has specific interactions.
- Platform behavior: use temporary directories, conditional symlink cases, fake
  executables instead of shell scripts, explicit deadlines, and no long sleeps.
- Existing or newly appearing workspace changes are authoritative and must be
  preserved.

## Coverage Matrix

### Host surfaces: `ai`, `help`, `plugins`

- Complete text and JSON manuals/help.
- Plugin registry field sets, deterministic ordering, and state filters.
- Enable, disable, reset, idempotency, unknown domains, and invalid plugins.
- Quiet and error diagnostics where behavior is domain-specific.

### Core: `ah-plugin-api`, `ah-runtime`

- ABI/version compatibility and capability validation.
- Invocation/global-option normalization and opaque argument preservation.
- Response allocation/ownership and malformed response handling.
- Dynamic loader ordering, conflicts, disabled state, invalid libraries, and panic
  isolation.

### Root supporting internals

- CLI normalization, dynamic-domain routing, global options, and opaque suffixes.
- Configuration paths, plugin-state persistence, and executable-relative discovery.
- Text-file safety at NUL, malformed UTF-8, and prefix-boundary cases.
- Git porcelain parsing for unusual paths, renames, and malformed records.
- HTTP expectation/interpolation parsers and bounded process-output readers where
  the public behavior benefits from a narrow unit-level contract.

### Process and state: `run`, `task`, `git`

- Exit status, bounded stdout/stderr, timeout and process-tree termination.
- Cwd application and preservation of global-looking child arguments.
- Task persistence, validation, listing, execution, unknown tasks, limits, and
  timeouts.
- All Git commands with normal repositories, unusual paths, renames, empty/unborn
  repositories, invalid references, and non-repository paths.

### Content discovery: `search`, `ctx`, `project`

- Complete JSON schemas and deterministic text output.
- Literal/regex, context, multiple roots, ignores, binary/large files, limits,
  symlinks, and Unicode columns.
- Context presets, symbol extraction, pack summaries, changed-file behavior, and
  non-Git paths.
- Empty and broad sample projects, ecosystems, roles, tools, grouped files,
  versions, commands, and platform-specific path handling.

### Built-in network client: `http`

- `request`, `get`, `post`, and `replay` request construction.
- Method, path, query, headers, bearer/basic auth, text/JSON/file bodies.
- Status, header, body, and JSON expectations.
- YAML/JSON specs, variables, fail-fast, and text/JSON/JUnit reports.
- Timeout, malformed response, oversized response, and assertion failures.

### Ollama plugin

- `ask` and `chat` request/response contracts.
- URL normalization, system prompts, JSON/text/quiet output, and metrics.
- HTTP failures, malformed/empty responses, oversized bodies, and timeouts.
- Race-resistant loopback server behavior with bounded deadlines.

### GitHub and GitLab plugins

- Every public command and subcommand.
- Request method/path/query/auth/body, pagination, filters, and limits.
- Complete text/JSON/quiet response contracts.
- Malformed, oversized, partial, and upstream error responses.
- Archive/trace limits and polling deadlines for workflow/pipeline commands.

### PostgreSQL plugin

- Early cross-platform feasibility spike for a fake `psql` executable.
- Tool resolution and explicit tool selection without real downloads.
- Connection flags, environment handling, argv/stdin, and generated SQL.
- Read-only guards and explicit confirmation for mutations and `EXPLAIN ANALYZE`.
- Metadata/list/describe/query/exec/explain/diagnostic command decoding.
- Complete text/JSON/error contracts without a live database.

## Execution Order

1. Preserve and commit the completed `file` reference tests.
2. Record and commit this campaign design.
3. Run the fake-`psql` feasibility spike. If a production seam is required, record
   the limitation and request a separate decision.
4. Complete `ai`, `help`, `plugins`, root supporting internals, `ah-plugin-api`,
   and `ah-runtime`.
5. Complete `run`, `task`, and `git`.
6. Complete `search`, `ctx`, and `project`.
7. Complete `http`.
8. Complete `ollama`, `github`, and `gitlab`.
9. Complete PostgreSQL coverage allowed by the approved process boundary.
10. Run final workspace and release validation.

Each module or tightly coupled group gets a focused diff, targeted validation, a
separate commit, and a basic-memory milestone.

## Defect Handling

When a new test exposes a mismatch with a documented released contract:

1. Do not weaken the expected contract to match the defect.
2. Record expected versus actual behavior and a minimal reproduction.
3. Mark that contract-matrix item blocked without committing a red test.
4. Continue independent modules.
5. Change production code only after explicit approval.

## Validation

During iteration, use the smallest applicable test command, for example:

```text
ah run check cargo test --test integration <module>::
ah run check cargo test --locked -p <package> --lib
```

After each group:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

For ABI/runtime increments and final handoff:

```text
ah run check cargo build --release --locked
```

## Definition of Done

- Every public command has an explicit contract-matrix disposition.
- Applicable text and JSON success paths are covered.
- Relevant invalid, failure, boundary, safety, limit, quiet, cwd, and platform
  cases are covered or have a documented exception.
- API plugins validate complete outbound requests and stable inbound contracts.
- PostgreSQL validates process and SQL behavior without a real server to the extent
  allowed by the approved production boundary.
- The default suite performs no live network requests, downloads, or database
  connections.
- Targeted tests, formatting, workspace tests, debug build, and final release build
  pass.
