# Testing

AIHelper tests treat released command output and structured responses as public
contracts. Add success, failure, and boundary coverage whenever behavior changes.

## Test Layers

Domain-level CLI contracts belong in `tests/integration/<domain>.rs`. These tests
execute the compiled `ah` binary with `assert_cmd` so argument parsing, runtime
dispatch, output rendering, and process exit behavior are covered together.

Keep focused unit tests for internal parsing and state transitions. Use black-box
integration tests for behavior visible to CLI and agent consumers.

## Contract Assertions

- Compare deterministic text output exactly.
- Parse JSON and assert stable field sets, values, and types explicitly.
- Cover success, relevant failure paths, limits, truncation, and boundary values.
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
