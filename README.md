# AIHelper

AIHelper is a Rust CLI toolbox for AI agents and developers.

The binary command is `ah`.

## Status
- Project is in bootstrap stage.
- `file` domain baseline is implemented (`read`, `head`, `tail`, `stat`, `tree`).
- `search` domain baseline is implemented (`text`, `files`).
- `ctx` domain baseline is implemented (`pack`, `symbols`, `changed`).
- `git` domain baseline is implemented (`changed`, `diff`, `blame`).
- `task` domain baseline is implemented (`save`, `list`, `run`).

## Quick Start
```bash
cargo build
cargo run --bin ah -- --help
cargo run --bin ah -- file read roadmap.md -n --from 1 --to 40
```

## Project Layout
- `src/` application source code
- `src/commands/` CLI domains (`file`, `search`, `ctx`, `git`, `task`)
- `tests/` integration and smoke tests
- `docs/agents/` token-efficient recipes for AI agents
- `docs/developers/` architecture and contribution docs
- `docs/reference/` command reference

## Documentation
- [Agents guide](docs/agents/README.md)
- [Developer guide](docs/developers/README.md)
- [Command reference](docs/reference/README.md)

## Roadmap
See [roadmap.md](roadmap.md).
