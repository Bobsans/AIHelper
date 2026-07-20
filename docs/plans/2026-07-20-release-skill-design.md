# Release Skill Design

## Goal

Add a project-local release skill shared by Codex, Claude, and OpenCode that
orchestrates AIHelper releases without turning a request to inspect or prepare a
release into authority to publish it.

## Structure

- Store the canonical skill at `.agents/skills/release/SKILL.md`.
- Use short discovery adapters at `.claude/skills/release/SKILL.md` and
  `.opencode/skills/release/SKILL.md`. Each adapter keeps compatible frontmatter
  and directs the client to load the canonical file.
- Mention the canonical skill in `AGENTS.md` and use a minimal `CLAUDE.md` to
  connect Claude to the same repository instructions.
- Avoid symlinks and junctions so an ordinary Git clone works consistently on
  Windows, Linux, and macOS.
- Keep authority gates, phase ordering, stop conditions, and completion criteria
  in the canonical skill only.
- Keep detailed release commands and release-note guidance in
  `docs/agents/recipes/release.md` and
  `docs/agents/recipes/release-notes.md` to avoid maintaining duplicate release
  procedures.

## Behavior

- Classify each request as inspection, preparation, or publication before taking
  action.
- Allow local version, lockfile, and changelog edits only in preparation mode.
- Require an explicit request to publish or complete the release before commits,
  pushes, workflow dispatches, tags, or GitHub Release mutations.
- Stop on dirty-tree ambiguity, failed validation, an unsynchronized release
  commit, missing artifacts, an existing tag or Release, or mismatched commit
  identities.
- Never enable the GitHub plugin or overwrite public release state without
  explicit approval.
- Stop if an adapter cannot load the canonical skill rather than reconstructing
  the release process from partial instructions.

## Validation

- Check that all three frontmatter blocks use the same name and description and
  that the name matches each skill directory.
- Check that each adapter resolves to the canonical file and does not duplicate
  the full release process.
- Audit the skill against both release recipes, including all authority gates,
  expected platform archives, validation commands, and verification criteria.
- Inspect the final Git diff and keep unrelated files unchanged.
