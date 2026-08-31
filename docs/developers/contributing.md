# Contributing

## Prerequisites

- `rustup` (the toolchain version is pinned by `rust-toolchain.toml` and is
  installed automatically on the first `cargo` invocation)
- `cargo` available in PATH
- Git

The workspace MSRV is declared once in `[workspace.package]` and is lower than
the pinned toolchain. CI verifies it separately, so a change that relies on a
newer language feature must raise `rust-version` deliberately.

Dependency versions are declared once in `[workspace.dependencies]`. Members
inherit them with `dep.workspace = true` and may add features on top; they must
not pin a different version.

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
- Keep command behavior domain-scoped and follow the existing error and output
  conventions.
- Keep changes focused: do not edit generated output or unrelated files.
- Updater on-disk journals and cross-process handoffs must interoperate across at
  least one release in both directions. A field added since v1 must be
  `skip_serializing_if`, because an older reader rejects a document carrying a
  field it does not know.
- A security check may be deduplicated, never narrowed. Where two checks differ,
  the union of them is the requirement; a shared helper that performs fewer
  checks than one of its callers did is a regression, not a cleanup.

## Suggested workflow

1. Implement command behavior in its domain module.
2. Add/extend integration tests in `tests/`.
3. Update command reference and AI recipe docs.
4. Verify both legacy CLI and typed invocation behavior.
5. Run the applicable checks before opening a pull request.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo build --locked
```

For release-sensitive changes, also run:

```bash
cargo build --release --locked
```

When a change alters the command catalog, the manuals, the CLI help tree, or an
error code, the golden snapshots in `tests/snapshots/` will fail. Regenerate and
review them as part of the change:

```bash
AH_UPDATE_SNAPSHOTS=1 cargo test --lib snapshots
```

A pull request that updates a snapshot must say why in its description. An
unexplained snapshot diff is a regression until proven otherwise.

When dependencies change, also run:

```bash
cargo deny --workspace check
```

Keep pull requests focused, explain the motivation and compatibility impact,
link related issues, and list the exact validation performed. Follow the
[Code of Conduct](../../CODE_OF_CONDUCT.md), and report vulnerabilities through
the [security policy](../../SECURITY.md) instead of a public issue.
