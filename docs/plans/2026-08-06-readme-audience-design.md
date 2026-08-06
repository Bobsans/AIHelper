# README Audience Design

## Goal

Turn the repository README into an accurate product entry point that attracts
both AI-agent users and CLI/DevOps developers before the v1.2.0 release.

## Approved Direction

Use a product-first hybrid structure. Lead with the user outcome and a short,
working demonstration, then provide installation and MCP setup. Follow with the
technical depth needed to evaluate the CLI, plugin runtime, safety model,
managed service, and signed updater.

## Audience

- AI-agent users who need predictable tools in Codex, Claude, Cursor, or another
  MCP client.
- CLI and DevOps developers who need deterministic text and JSON commands for
  repository, HTTP, database, release, and automation workflows.

## Structure

1. Product name, concise value proposition, and truthful badges.
2. Short explanation of the problem AIHelper solves.
3. A small real-command demonstration.
4. Binary installation and source-build paths.
5. MCP stdio and HTTP quick starts.
6. Capability overview grouped by workflow instead of an exhaustive command
   dump.
7. Safety, deterministic output, plugins, managed Windows service, and signed
   update behavior.
8. Platform support and explicit limitations.
9. Architecture, documentation, contribution, and release links.

## Content Rules

- Keep the README in English for the broadest developer audience.
- Ground every feature and compatibility claim in current code, documentation,
  release workflow, or v1.2.0 release notes.
- Prefer copyable commands and links to detailed references over duplicating
  reference documentation.
- Do not claim benchmark results, broad client certification, or open-source
  licensing without evidence.
- Omit a license badge until the repository contains a license.
- Use only GitHub-native or Shields.io badges that require no registration.

## Validation

- Inspect the rendered Markdown structure and all local links.
- Check command examples against the release binary or documented CLI surface.
- Run `git diff --check` and the repository formatting check.
- Preserve all existing release-preparation changes and do not commit, push, or
  publish without separate Publish authority.
