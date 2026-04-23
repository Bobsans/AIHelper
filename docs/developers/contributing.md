# Contributing

## Prerequisites
- Rust stable toolchain
- `cargo` available in PATH

## Local Development
```bash
cargo fmt
cargo test
cargo run --bin ah -- --help
```

## Standards
- Keep command output deterministic.
- Add tests for both success and failure cases.
- Update docs in `docs/agents` and `docs/reference` for every user-facing command change.
- Preserve stable JSON field names after release.

## Suggested Workflow
1. Implement command behavior in its domain module.
2. Add/extend integration tests in `tests/`.
3. Update command reference and AI recipe docs.
4. Run format and test checks before commit.
