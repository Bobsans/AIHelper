# Community Standards Design

## Goal

Complete the GitHub Community Standards checklist for AIHelper with useful,
low-maintenance policies and templates that welcome both AI-agent users and
CLI/DevOps developers. Keep the work inside the prepared v1.2.0 release scope
without publishing or changing GitHub settings during the Prepare phase.

## Decisions

- License: MIT.
- Copyright holder: `Copyright (c) 2026 Bobsans`.
- Conduct-report contact: `mr.bobsans@gmail.com`.
- Security-report channel: GitHub Private Vulnerability Reporting.
- Community documents are written in English for the repository's global
  audience.
- Existing detailed contributor instructions remain canonical at
  `docs/developers/contributing.md`.
- The GitHub-recognized root `CONTRIBUTING.md` is a concise gateway, not a copy
  of the detailed guide.
- Blank issues remain enabled for unusual integration or support cases that do
  not fit the structured forms.

## Repository Layout

```text
LICENSE
CODE_OF_CONDUCT.md
CONTRIBUTING.md
SECURITY.md
.github/
  ISSUE_TEMPLATE/
    bug_report.yml
    feature_request.yml
    config.yml
  PULL_REQUEST_TEMPLATE.md
```

`README.md` is updated only to add an MIT badge and compact links to the four
root community documents.

## Document Contracts

### `LICENSE`

Use the unmodified standard MIT License text with the approved copyright line.
Do not add project-specific clauses, exceptions, or attribution requirements.

### `CODE_OF_CONDUCT.md`

Use the exact Contributor Covenant 2.0 body returned by GitHub's
`/codes_of_conduct/contributor_covenant` endpoint. Replace only
`[INSERT CONTACT METHOD]` with `mr.bobsans@gmail.com` so GitHub recognizes the
document as its supported template. Conduct reports must be handled privately;
the policy must not direct reporters to public issues.

### `CONTRIBUTING.md`

Provide a short, useful entry point that:

- welcomes bug reports, feature proposals, documentation changes, code changes,
  and plugin contributions;
- sends vulnerabilities to `SECURITY.md` instead of public issues;
- sends conduct incidents to `CODE_OF_CONDUCT.md`;
- directs bugs and features to the matching issue forms;
- links to `docs/developers/contributing.md` for prerequisites, development
  workflow, required checks, compatibility rules, and documentation standards;
- asks contributors to keep pull requests focused and explain validation.

Do not duplicate build commands, architecture guidance, command reference, or
the full engineering checklist from the developer guide.

### `SECURITY.md`

State that only the latest published GitHub Release is supported with security
updates and that older versions may be asked to reproduce on the latest release.
Use `https://github.com/Bobsans/AIHelper/security/advisories/new` as the sole
private reporting channel. Explicitly prohibit reporting vulnerability details
in public issues, discussions, or pull requests.

Ask reports to include affected AIHelper version, platform and architecture,
installation source, affected CLI/MCP/plugin/updater surface, impact,
reproduction steps, and sanitized diagnostics. Ask reporters to remove secrets
and personal data. Promise coordinated, private handling but no response-time,
fix-time, bounty, or disclosure SLA.

Private Vulnerability Reporting must be enabled before the policy reaches the
default branch so the reporting link works.

## Issue Forms

### `bug_report.yml`

Collect:

- concise bug summary;
- affected surface: CLI, MCP stdio, MCP HTTP, managed MCP service, updater,
  plugin, packaging, documentation, or other;
- exact `ah --version` output;
- operating system and architecture;
- installation source;
- reproduction steps;
- expected and actual behavior;
- optional sanitized logs or diagnostics;
- required confirmations that the report is not a private security issue, that
  secrets were removed, and that the Code of Conduct is accepted.

Do not reference repository labels or assignees whose existence has not been
verified.

### `feature_request.yml`

Collect:

- problem or workflow being improved;
- primary audience: AI agent/MCP, CLI automation, DevOps, plugin development,
  documentation, or other;
- desired outcome;
- alternatives or current workaround;
- optional compatibility or automation impact;
- required Code of Conduct confirmation.

Keep the form focused on user outcomes; it is not a PRD or implementation plan.

### `config.yml`

- Set `blank_issues_enabled: true` so unusual questions are not blocked.
- Add a contact link to GitHub Private Vulnerability Reporting.
- Add a contact link to the command-reference documentation.
- Do not add Discussions, support services, or other channels that are not
  enabled and maintained.

## Pull Request Template

Use one `.github/PULL_REQUEST_TEMPLATE.md` with:

- summary;
- motivation and optional linked issue;
- validation performed;
- compatibility and documentation notes;
- a short checklist covering focused scope, success/failure tests where behavior
  changes, required docs, deterministic output and stable CLI/JSON/MCP/plugin ABI
  contracts, and removal of secrets or sensitive logs.

The template links to `CONTRIBUTING.md` instead of repeating its guidance.

## README Changes

- Add an MIT badge linked to `LICENSE`.
- Extend the Documentation or Contributing area with direct links to
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and `LICENSE`.
- Keep policy details outside README.

## GitHub Repository Settings

The proposed repository description is:

> Safe, predictable Rust CLI and typed MCP tools for AI agents and developer
> automation — deterministic output, bounded execution, plugins, and signed
> updates.

Before the community files reach `main`:

1. Set the repository description to the approved text.
2. Enable GitHub Private Vulnerability Reporting.
3. Confirm GitHub Issues remain enabled.

These are external mutations and are not authorized by the current release
Prepare phase. They require separate explicit authority and must not be performed
during local implementation.

## Validation

Local validation must prove:

- every expected path exists with exact GitHub-recognized casing;
- the MIT text and copyright line are exact;
- the Code of Conduct matches GitHub's Contributor Covenant 2.0 template except
  for the approved contact replacement;
- issue forms and `config.yml` parse as YAML; each issue form includes the
  required top-level `name`, `description`, and `body`, and its field IDs are
  unique;
- all relative Markdown links resolve;
- no placeholder, `TODO`, `TBD`, invented label, secret, or unsupported promise
  remains;
- the existing dirty working-tree changes are preserved;
- `git diff --check` and `cargo fmt --all -- --check` pass.

The previously completed workspace tests and debug/release builds remain valid
because this change touches only documentation and GitHub templates. If any Rust
or workflow file changes during implementation, rerun all required release
checks.

After a separately authorized commit and push to the default branch, verify:

- the GitHub Community Profile recognizes Description, README, Code of Conduct,
  Contributing, License, Security Policy, Issue Templates, and Pull Request
  Template;
- GitHub detects the license as MIT;
- the issue chooser renders both forms and both contact links;
- new pull requests receive the template;
- the private vulnerability reporting link opens the private advisory form.

## Out of Scope

- `SUPPORT.md`, `FUNDING.yml`, `CODEOWNERS`, CLA, DCO, signed-commit rules,
  Discussions, bots, label creation, and automated template tests;
- Cargo license inheritance or crates.io publishing metadata;
- commits, pushes, workflow dispatches, tags, GitHub Releases, or repository
  setting changes during the Prepare phase.
