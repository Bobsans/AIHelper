# Architecture Overview

## High-Level
- Entry point: `src/main.rs`
- Runtime glue: `src/lib.rs`
- CLI parsing and global flags: `src/cli.rs`
- Error model: `src/error.rs`
- Output helpers: `src/output.rs`
- Domain handlers: `src/commands/*`

## Domain Structure
Each domain module owns:
- clap argument structs
- subcommand enum
- `execute(...)` dispatcher
- domain-specific implementation

Current domains:
- `file`
- `search`
- `ctx`
- `git`
- `task`

## Output Contract
- Text output for humans by default
- `--json` for machine-readable output
- `--quiet` to suppress standard output

## Error Contract
- Centralized in `AppError`
- Stable string code per error family
- Non-zero process exit on failure
