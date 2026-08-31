# AGENTS.md

## Scope

These instructions apply to the entire repository.

## Working Context

- Use the `aihelper` basic-memory project for repository context and record durable decisions or completed milestones there.
- Start unfamiliar work with `ah ai info`, optionally narrowed with `--domain`.
- Prefer `ah` commands over ad-hoc shell scripts for repository inspection, search, Git context, project detection, and checked command execution.
- Treat changes that appear in files while you are working as authoritative user changes. Preserve them and adapt your work; do not revert or overwrite them.
- Inspect the working tree before editing and keep unrelated changes untouched.

## Skills

- Project skills are canonical under `.agents/skills/`. Client-specific skill
  files under `.claude/skills/` and `.opencode/skills/` are discovery adapters;
  do not duplicate or maintain the full instructions there.
- For release inspection, preparation, publication, or verification, load and
  follow `.agents/skills/release/SKILL.md`.

## Project Overview

AIHelper is a Rust workspace that provides the `ah <domain> <command>` CLI. It
uses a plugin-oriented architecture with in-process dispatch.

- `src/`: the root binary - CLI, bootstrap, built-in plugin adapters, host
  commands, and the `upgrade`/`mcp service` routes.
- `crates/`: every library the binary is assembled from.
- `plugins/`: dynamic plugin crates.
- `tests/`: integration tests.
- `docs/agents/`: AI-oriented recipes.
- `docs/developers/`: architecture and contributor guidance.
- `docs/reference/`: user-facing command reference.
- `docs/decisions/`: architecture decision records.

Which crate owns what is in
[`docs/developers/architecture.md`](docs/developers/architecture.md). Read it
before changing a runtime contract; do not restate its crate list here.

## Development Rules

The repository standards, the compatibility rules, and the required checks are
stated once, in
[`docs/developers/contributing.md`](docs/developers/contributing.md). Follow
them. Two agent-specific additions:

- Run each check through `ah run check` rather than a bare shell invocation, so
  the run is bounded and its output is captured.
- Report any check that could not be run, and why, instead of omitting it.

Use the smallest relevant check while iterating, then run the applicable
workspace checks before handoff.
