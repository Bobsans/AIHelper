# File Integration Tests Design

## Context

The `file` domain already has 19 passing black-box integration tests in
`tests/integration/file.rs`. They cover every implemented subcommand, but several
assertions only check output fragments, `head` and `tail` lack JSON contract
coverage, and failure and boundary cases are unevenly distributed.

The next test increment will treat the released CLI output as a public contract.
It will strengthen coverage without changing production behavior unless a test
exposes a documented contract defect that is approved separately.

## Goals

- Cover the text and JSON contracts of `read`, `head`, `tail`, `stat`, and `tree`.
- Verify relevant failure paths and file-safety behavior through the real `ah`
  binary.
- Keep assertions deterministic and portable across Windows and Unix.
- Preserve the meaning of the existing tests while removing needless
  duplication.

## Non-goals

- Testing generic Clap parsing already covered by other integration modules.
- Exact comparisons of filesystem timestamps or other OS-controlled metadata.
- Permission-denied scenarios that depend on user privileges and host policy.
- Introducing snapshot dependencies or a custom scenario DSL.

## Test Architecture

Tests remain in `tests/integration/file.rs` and execute the compiled `ah` binary
with `assert_cmd`. Small local helpers may run a file command, decode JSON, and
verify exact field sets. `tests/integration/common.rs` remains limited to helpers
that are genuinely reusable across domains, such as conditional symlink setup.

JSON assertions parse `serde_json::Value` and verify stable field names, values,
and types. Text output is compared exactly when deterministic. Paths are derived
from temporary fixtures instead of hard-coding platform separators. Timestamp
fields are validated as a number or `null`, not against a specific instant.

## Coverage Matrix

### `file read`

- Full-file and ranged reads, with and without line numbers.
- Empty files and ranges beyond end-of-file.
- Complete JSON payload and `--limit`/`truncated` behavior.
- Missing paths, directory paths, binary/non-UTF8 content, `--max-bytes`, and
  symlink follow/no-follow behavior.

### `file head`

- Explicit and default line counts, numbering, and zero lines.
- Complete JSON payload and interaction between `--lines` and `--limit`.
- Read errors and the applicable binary, size, and symlink safety policy.

### `file tail`

- Explicit and default line counts with original source line numbers.
- Zero lines, complete JSON payload, and `--limit` behavior.
- Read errors and the applicable binary, size, and symlink safety policy.

### `file stat`

- Exact deterministic text fields and complete JSON field/type contract.
- File, directory, and symlink kinds.
- Missing-path failure.

### `file tree`

- Exact text rendering and complete flattened JSON entries.
- Deterministic ordering, depth zero and one, and a single-file root.
- Default path under global `--cwd`.
- `--limit`/`truncated`, missing paths, symlink traversal, and cycle safety.

## Platform Policy

Fixtures use `tempfile`. Symlink cases remain conditional when the host does not
permit link creation, which is common on Windows without Developer Mode or
elevated privileges. Tests avoid assumptions about path separators, creation
time support, or permission semantics.

## Implementation Plan

1. Add narrow local helpers for command execution, JSON decoding, and exact JSON
   field-set checks.
2. Strengthen existing `read`, `head`, and `tail` success assertions and add the
   missing JSON, empty, range, limit, and default-value scenarios.
3. Add command-level failure and safety cases without duplicating generic CLI
   parser tests.
4. Complete `stat` text/JSON and kind coverage.
5. Complete `tree` rendering, ordering, depth, cwd, limit, missing-path, symlink,
   and cycle coverage.
6. Refactor repeated fixture setup only where the resulting helper remains
   simpler than the individual tests.
7. Run targeted checks while iterating, followed by the required workspace
   validation.

## Validation

```text
ah run check cargo test --test integration file::
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

## Acceptance Criteria

- All five `file` subcommands have success coverage for both text and JSON output.
- Each command has the relevant failure and boundary coverage.
- Released JSON field names and types are asserted explicitly.
- Tree ordering and truncation are deterministic and covered.
- Tests pass on Windows and Unix without unstable timestamp or permission checks.
- No production behavior changes are bundled into the test-only change unless a
  separately confirmed defect requires one.
