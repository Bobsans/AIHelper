# Testing

AIHelper tests treat released command output and structured responses as public
contracts. Add success, failure, and boundary coverage whenever behavior changes.

## Test Layers

Domain-level CLI contracts belong in `tests/integration/<domain>.rs`. These tests
execute the compiled `ah` binary with `assert_cmd` so argument parsing, runtime
dispatch, output rendering, and process exit behavior are covered together.

Keep focused unit tests for internal parsing and state transitions. Use black-box
integration tests for behavior visible to CLI and agent consumers.

Core crates use focused unit tests for ABI and version compatibility, request
normalization, response ownership, loader behavior, persisted state, and panic
isolation. Duplicate those cases at the integration layer only when the behavior
crosses the public CLI boundary.

Build workspace coverage as vertical domain slices. Each slice should exercise the
applicable public text, JSON, error, boundary, safety, limit, and global-option
contracts while keeping the workspace green independently.

## Contract Assertions

- Compare deterministic text output exactly.
- Parse JSON and assert the exact set of stable fields, values, and types.
- Cover success, relevant failure paths, limits, truncation, and boundary values.
- For errors, assert stable diagnostic codes, messages, and hints rather than nested
  platform error wording.
- For bounded output, assert returned counts, truncation metadata, omitted content,
  and warning behavior in text and JSON modes where applicable.
- Cover `--json`, `--quiet`, `--limit`, and `--cwd` where a domain has behavior
  specific to those global options.
- For byte-bounded UTF-8 input, test 2-, 3-, and 4-byte characters split at the
  exact boundary separately from definite malformed sequences and NUL data.
- Prefer semantic assertions over snapshots or a custom scenario DSL.
- Do not weaken existing assertions when consolidating repeated setup.

## Portable Fixtures

Use `tempfile` fixtures and derive expected paths from the fixture instead of
hard-coding platform separators. Validate OS-controlled timestamps by type, such
as a number or `null`, rather than by exact value.

Avoid permission-dependent cases whose result changes with user privileges or
host policy. Symlink tests may be skipped when the platform cannot create the
required link, which is common on Windows without Developer Mode or elevation.

## Hermetic Integrations

The default test suite must stay offline and require no credentials, live database,
downloads, elevated privileges, or fixed host ports.

Test HTTP-based plugins with deterministic loopback servers bound to port zero.
Assert the complete outbound method, path, query, headers, and body before checking
the stable response contract. Use explicit deadlines and avoid long sleeps.

Test process-based integrations with cross-platform fake executables where the
production boundary permits it. Prefer executable fixtures over shell scripts so
the same contract can run on Windows and Unix. If a command cannot be isolated
without a new production seam, document the uncovered boundary and make that seam
a separate design decision.

## Typed Command Contracts

Treat each typed command as one public contract shared by CLI and MCP:

- define closed input and output DTOs with object-root JSON Schemas, preserving
  released JSON field names and nesting;
- provide complete effects, risk, impact, and reversibility metadata;
- route CLI and MCP through the same typed handler, with human-readable text
  rendering tested separately;
- reserve top-level input property `context` for the MCP adapter;
- keep stable diagnostic codes across adapters and failure paths.

Catalog and parity tests must prove that every bundled command except
`mcp.serve` appears exactly once, exposed `ah.*` tool names are deterministic and
unique, schemas and effects are complete, and equivalent CLI JSON and MCP
structured output match. Enabling or disabling a typed plugin must update the
live tool list. MCP transport tests must also prove that command output never
writes non-protocol bytes to stdout.

## Runtime and MCP Concurrency

Use barriers, channels, or condition variables for lifecycle tests instead of
timing-only assertions. Cover unique execution IDs, reused protocol request IDs,
pre-delivered cancellation, panic-safe cleanup, prompt logical timeout/cancellation,
and physical capacity retention by uncooperative handlers.

Registry tests should prove that definitions and validators compile once per
revision, state revisions change only for real mutations, and concurrent readers
see a complete old or new tool snapshot. Exercise accepted and rejected root-schema
forms, reserved host domains, and optional `context` injection.

Persistence tests cover failed writes without live-state publication, concurrent
writers preserving unrelated updates, parseable old-or-new reads, bounded lock
timeouts, and temporary-file cleanup. Search regressions cover exact context at the
first, middle, and last lines, sentinel truncation, cancellation during scanning,
duplicate roots, and allocation bounded by the requested result limit.

## Helper Scope

Keep command execution, JSON decoding, and assertion helpers local to the domain
test module while they are domain-specific. Move a helper to
`tests/integration/common.rs` only when multiple domains genuinely reuse it and
the shared abstraction remains simpler than the individual tests.

## Change Isolation

A test-only change must not alter production behavior. If stronger coverage
reveals a contract defect, confirm and fix that defect as a separate behavioral
change with its own success and failure coverage and documentation updates.

## Validation

Run the narrow integration target while iterating, then the required workspace
checks:

```text
ah run check cargo test --test integration <domain>::
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```
