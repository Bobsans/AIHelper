# Contributing

## Prerequisites

- Rust stable toolchain
- `cargo` available in PATH
- Git

## Local development

```bash
cargo fmt
cargo test
cargo run --bin ah -- --help
```

## Standards

- Keep command output deterministic.
- Add tests for both success and failure cases.
- Update `docs/agents` and `docs/reference` for every user-facing command change.
- Preserve released CLI behavior, JSON field names, MCP contracts, and plugin ABI
  compatibility unless the change explicitly requires a breaking release.
- Add or update the typed command descriptor, schemas, effects, and MCP tests
  whenever a command is added or its behavior changes.
- Resolve paths and child process working directories from the invocation
  context; do not mutate the process-global cwd.
- Treat every handler as concurrent: multiple calls to the same command may
  overlap, and cancellation may run concurrently with invocation. Protect
  shared state and make polling loops and child processes cancellation-aware.

## Suggested workflow

1. Implement command behavior in its domain module.
2. Add/extend integration tests in `tests/`.
3. Update command reference and AI recipe docs.
4. Verify both legacy CLI and typed invocation behavior.
5. Run the applicable checks before opening a pull request.

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets --locked
cargo build --locked
```

For release-sensitive changes, also run:

```bash
cargo build --release --locked
```

Keep pull requests focused, explain the motivation and compatibility impact,
link related issues, and list the exact validation performed. Follow the
[Code of Conduct](../../CODE_OF_CONDUCT.md), and report vulnerabilities through
the [security policy](../../SECURITY.md) instead of a public issue.
