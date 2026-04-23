# AIHelper Roadmap

## 1) Vision
Build `ah` as a local CLI toolbox for AI agents and developers.  
Primary goal: reduce context usage and repetitive shell boilerplate while keeping operations explicit, fast, and scriptable.

## 2) Product Goals
- Replace long ad-hoc shell commands with stable short commands (example: `ah file read <path> -n`).
- Standardize outputs so AI can parse them predictably (`text`, `json`, compact summaries).
- Minimize token usage by returning only needed slices, metadata, and focused context.
- Keep tools safe by default (path checks, explicit overwrite flags, dry-run where relevant).
- Maintain first-class documentation for both AI agents and human developers.

## 3) Non-Goals (initially)
- No remote cloud dependency for core commands.
- No full IDE replacement.
- No heavy UI until CLI core is stable.

## 4) Tech Stack Decision
- Primary language: `Rust`
- CLI foundation: `clap` (argument parsing) + `serde/serde_json` (`--json` output)
- Test baseline: `cargo test` + snapshot tests for stable CLI output

## 5) Core CLI Design
- Entry point: `ah <domain> <command> [options]`
- Domains (v1): `file`, `search`, `ctx`, `git`, `task`
- Common flags:
- `--json` machine-readable output
- `--quiet` minimal output
- `--cwd <path>` explicit working directory
- `--limit <n>` output cap for context control
- Error format:
- short human message
- stable error code
- optional `--json` error object

## 6) MVP Scope (Phase 1)

### `file`
- `ah file read <path> [-n] [--from N] [--to N]`
- `ah file head <path> [--lines N]`
- `ah file tail <path> [--lines N]`
- `ah file stat <path>`
- `ah file tree [path] [--depth N]`

### `search`
- `ah search text <pattern> [path] [--glob ...] [--ignore-case] [--context N]`
- `ah search files <query> [path]`

### `ctx` (context reduction utilities)
- `ah ctx pack <path...>`: returns compact structured digest (files, key ranges, symbols)
- `ah ctx symbols <path>`: headings/classes/functions only
- `ah ctx changed`: summarizes local changed files (when git exists)

## 7) Phase Plan

### Phase 0: Bootstrap (1-2 days)
- Initialize Rust CLI skeleton (`Cargo.toml`, `src`, `tests`, `docs`).
- Add command parser and domain-command registry.
- Add shared output formatter (`text/json`).
- Add smoke tests for command wiring.
- Create documentation skeleton (`docs/agents`, `docs/developers`, `docs/reference`).

### Phase 1: File + Search MVP (3-5 days)
- Implement `file read/head/tail/stat/tree`.
- Implement `search text/files` on top of fast backend (`rg` when available, fallback otherwise).
- Add deterministic line-number rendering and range slicing.
- Add tests for edge cases (UTF-8, large files, missing paths, binary detection).
- Write command docs with examples for every MVP command.

### Phase 2: Context Utilities (3-4 days)
- Implement `ctx pack`, `ctx symbols`, `ctx changed`.
- Add token-aware truncation strategy.
- Add presets for AI workflows (`--preset review`, `--preset debug`, `--preset summary`).

### Phase 3: Git + Task Helpers (3-4 days)
- `git` helpers: compact diff summaries, changed file groups, blame snippets.
- `task` helpers: save/load reusable command recipes for repetitive AI operations.

### Phase 4: Hardening + DX (ongoing)
- Performance profiling on large repositories.
- Better error diagnostics and docs.
- Package/distribution strategy and versioning policy.
- Automate docs validation in CI (link check + example command verification).

## 8) Priority Backlog
- P0:
- `file read` with line numbers and ranges
- `search text/files`
- stable `json` output
- P1:
- `ctx pack/symbols`
- git-aware context extraction
- presets for common AI tasks
- P2:
- recipe system (`task`)
- plugin API for custom domains
- optional TUI wrapper

## 9) Quality Gates
- Every command has:
- unit tests for success + failure cases
- snapshot tests for output format
- consistent non-zero exit codes on failure
- `ah --help` and per-command `--help`
- Documentation gate for every command:
- one short AI-focused recipe (copy/paste workflow)
- one developer-focused example with expected output
- JSON output example (when supported)
- Context efficiency target:
- at least 30-50% reduction in typical AI prompt payload vs raw file dumps

## 10) First Milestone Definition
Milestone `v0.1.0` is done when:
- `file` and `search` P0 commands are complete
- JSON output is stable
- docs include:
- quickstart
- AI recipes for core commands
- developer command reference for all `file`/`search` MVP features
- basic benchmark shows acceptable speed on medium-size repo

## 11) Immediate Next Steps
1. Scaffold Rust CLI skeleton and command registry.
2. Implement `ah file read` first as reference command.
3. Add golden tests for numbered/ranged output.
4. Prepare initial docs structure for AI + developers.
5. Prepare initial release workflow for `cargo` build/test.

## 12) Documentation Architecture
- `docs/agents/`
- goal: minimal-token usage guides for AI agents
- format: task recipes (`Goal`, `Command`, `Output shape`, `When to use`)
- `docs/developers/`
- goal: implementation and extension guidance
- format: architecture notes, contribution guide, testing and release flow
- `docs/reference/`
- goal: authoritative command reference
- format: one page per command with flags, examples, exit codes, and JSON schema notes
- `README.md`
- entrypoint with install, quickstart, and links to all docs sections
