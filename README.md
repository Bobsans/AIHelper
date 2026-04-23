# AIHelper

AIHelper is a Rust CLI toolbox for AI agents and developers.

The binary command is `ah`.

## Status
- Project is in bootstrap stage.
- Plugin-oriented runtime architecture is in place.
- Built-in domains are implemented as plugins:
- `file` (`read`, `head`, `tail`, `stat`, `tree`)
- `search` (`text`, `files`)
- `ctx` (`pack`, `symbols`, `changed`)
- `git` (`changed`, `diff`, `blame`)
- `task` (`save`, `list`, `run`)
- Plugin registry command is available: `ah plugins list`.

## Quick Start
```bash
cargo build
cargo run --bin ah -- --help
cargo run --bin ah -- plugins list
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
