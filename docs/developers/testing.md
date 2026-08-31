# Testing

AIHelper tests treat released command output and structured responses as public
contracts. Add success, failure, and boundary coverage whenever behavior changes.

## Test Layers

Domain-level CLI contracts belong in `tests/integration/<domain>.rs`. Prefer the
in-process harness (`src/harness.rs`, behind the `harness` feature the package
enables for itself as a dev-dependency): it builds the registry the shipped
binary builds, parses argv with the production parser, dispatches through the
production manager, and hands back what the command rendered. A test that only
asserts about argv in and rendered text out needs no subprocess.

Spawn the compiled binary only for what a process *is*, and say in a doc comment
which of these it is:

- the exit code, and which stream the text reached
- `--json` routing a refusal to stderr, which `AppError::print` decides from a
  process-global
- an environment variable that must be in `ah`'s own environment, or a replaced
  `PATH`
- a relative `--cwd`, which resolves against a process working directory
- crash recovery, the event log and its redaction, and the managed service

Child processes under test are not the same thing: `run check` spawns them,
bounds their output and kills their descendants, and that is the behavior being
measured.

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
- Prefer semantic assertions over snapshots or a custom scenario DSL when
  testing the behavior of a single command. Whole-artifact snapshots are a
  separate tool with a separate purpose; see **Golden Snapshots** below.
- Do not weaken existing assertions when consolidating repeated setup.

## Golden Snapshots

`tests/snapshots/` holds whole-artifact snapshots of the contracts that must not
move by accident:

| Snapshot | Covers |
|---|---|
| `typed-command-catalog.snap` | every typed descriptor: schemas, effects, examples, secret slots |
| `plugin-manuals.snap` | the manual returned by every enabled plugin |
| `cli-help.snap` | the rendered `--help` output of the whole command tree |
| `error-codes.snap` | the code and rendered message of every mapped error |

They are generated in-process by `src/snapshots.rs`, so they run as unit tests
and do not depend on a terminal, a git repository, or the plugin directory next
to the test binary.

Their purpose is refactoring safety, not behavior specification: a structural
change that is meant to preserve behavior must leave every snapshot byte-identical,
which turns "I believe this refactor is safe" into a check. They complement the
semantic assertions above and never replace them — a snapshot proves nothing
changed, while a semantic assertion proves the behavior is correct in the first
place.

Regenerate after an intentional change and review the diff:

```bash
AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots
```

State the reason for any snapshot diff in the pull request description.

Deriving a schema from its Rust type reorders the `required` array, which JSON
Schema treats as a set. That noise hides real diffs, so compare the catalog with
`required` canonicalised instead of reading `git diff`:

```bash
cp tests/snapshots/typed-command-catalog.snap target/catalog_prev.snap
AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots
python scripts/catalog_delta.py
```

Anything that prints is a genuine change to the published contract.

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
Use `ah-plugin-testkit` rather than a per-plugin mock server: three hand-written
copies had each drifted from the others, and the one missing a blocking-mode
call produced a Windows-only flake nobody could carry a fix back for.

## Property Coverage

The redaction engine and the two parsers carry `proptest` properties on the
pinned stable toolchain, so they run on every CI platform. A `cargo-fuzz` target
would need a nightly toolchain and a CI lane of its own and is deliberately not
set up; what is missing there is coverage guidance, not the leak property
itself.
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
