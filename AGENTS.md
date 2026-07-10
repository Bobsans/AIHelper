# AGENTS.md

## Scope

These instructions apply to the entire repository.

## Working Context

- Use the `aihelper` basic-memory project for repository context and record durable decisions or completed milestones there.
- Start unfamiliar work with `ah ai info`, optionally narrowed with `--domain`.
- Prefer `ah` commands over ad-hoc shell scripts for repository inspection, search, Git context, project detection, and checked command execution.
- Treat changes that appear in files while you are working as authoritative user changes. Preserve them and adapt your work; do not revert or overwrite them.
- Inspect the working tree before editing and keep unrelated changes untouched.

## Project Overview

AIHelper is a Rust workspace that provides the `ah <domain> <command>` CLI. It uses a plugin-oriented architecture with in-process dispatch.

- `src/`: root CLI, runtime bootstrap, built-in plugin adapters, and core domain implementations.
- `crates/ah-plugin-api/`: stable plugin request/response and C ABI contracts.
- `crates/ah-runtime/`: plugin registry, manager, and dynamic loader.
- `plugins/`: dynamic plugin crates.
- `tests/`: integration tests.
- `docs/agents/`: AI-oriented recipes.
- `docs/developers/`: architecture and contributor guidance.
- `docs/reference/`: user-facing command reference.

## Development Rules

- Keep text and JSON output deterministic.
- Preserve released JSON field names and plugin ABI compatibility unless an explicit breaking change is requested.
- Add or update tests for both success and failure paths when behavior changes.
- For user-facing command changes, update the relevant command reference and AI recipe documentation.
- Keep command behavior domain-scoped and follow existing error and output conventions.
- Prefer focused changes; do not modify generated output or unrelated files.

## Validation

Use the smallest relevant check while iterating, then run the applicable workspace checks before handoff:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
```

For release-sensitive work, also run:

```text
ah run check cargo build --release --locked
```

Report any check that could not be run and why.
