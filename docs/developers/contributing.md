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
- Add or update the typed command descriptor, schemas, effects, and MCP tests
  whenever a command is added or its behavior changes.
- Resolve paths and child process working directories from the invocation
  context; do not mutate the process-global cwd.
- Treat handlers as future parallel work: protect shared state and implement
  cancellation for polling loops and child processes where practical.

## Suggested Workflow
1. Implement command behavior in its domain module.
2. Add/extend integration tests in `tests/`.
3. Update command reference and AI recipe docs.
4. Verify both legacy CLI and typed invocation behavior.
5. Run format and workspace test checks before commit.
