# Recipe: Write Changelog Entries And GitHub Release Notes

## Goal

Keep `CHANGELOG.md` concise, complete, and stable while turning the same verified
facts into a polished GitHub Release description that helps users understand the
value, compatibility, and available downloads.

The changelog is the durable source of truth. The GitHub Release is an editorial
presentation of that source, not an independent list of claims.

## Evidence And Accuracy

Ground every statement in at least one of these sources:

1. the final release diff;
2. the tagged `CHANGELOG.md` section;
3. completed validation or smoke-test results;
4. published workflow, release, and asset metadata.

Do not infer benefits that were not demonstrated. Do not promise future work,
claim a performance improvement without evidence, or describe an internal
refactor as a user feature. If compatibility or migration impact is uncertain,
resolve it before publication instead of using vague language.

## Changelog Categories

Use the Keep a Changelog categories below in this order. Include only non-empty
categories.

| Category | Content |
| --- | --- |
| `Added` | New commands, options, tools, integrations, supported platforms, or other capabilities. |
| `Changed` | Backward-compatible behavior, performance, reliability, packaging, or operational improvements. |
| `Deprecated` | Supported behavior that users should stop relying on before a future removal. |
| `Removed` | Features, commands, options, contracts, or support that no longer exist. |
| `Fixed` | User-visible defects, regressions, incorrect output, crashes, hangs, or platform-specific failures. |
| `Security` | Vulnerability fixes, hardening with user impact, secret handling, or security-relevant defaults. |

Breaking changes normally appear under `Changed` or `Removed` and start with
`**Breaking:**` so they cannot be missed. Security-sensitive details must explain
the user impact without publishing exploit instructions or secrets.

## Changelog Template

Use this structure for `[Unreleased]` and replace the heading when publishing.
Delete every empty category.

```markdown
## [Unreleased]

### Added

- <New user-visible capability and where to use it.>

### Changed

- <Behavior or operational improvement and its practical effect.>

### Deprecated

- <Deprecated surface, supported replacement, and expected removal boundary.>

### Removed

- <Removed surface and the migration path, when one exists.>

### Fixed

- <Incorrect behavior that is now corrected, including relevant platform or boundary.>

### Security

- <Security improvement and the action users should take, if any.>
```

A published section uses the release version and ISO date:

```markdown
## [X.Y.Z] - YYYY-MM-DD
```

## Writing Changelog Entries

- Lead with the observable outcome, not the implementation activity.
- Use one bullet for one coherent user or operator outcome.
- Combine its implementation, tests, and documentation into the same outcome;
  do not list them as separate changes.
- Name commands, flags, environment variables, files, JSON fields, and platforms
  with exact spelling and inline code formatting.
- State compatibility, migration, or minimum-platform impact in the same bullet
  when it changes.
- Prefer specific verbs such as `adds`, `preserves`, `rejects`, `reports`, or
  `terminates` over vague phrases such as `improves things`.
- Do not include commit hashes, commit prefixes, issue-tracker narration, or
  exhaustive internal refactoring details.
- Do not duplicate one change across categories. Choose the category that best
  represents its primary user impact.

Weak:

```markdown
- Refactored process code and added tests.
```

Strong:

```markdown
- Windows command timeouts now terminate descendant processes through a Job
  Object without scanning unrelated system threads.
```

## Transform Changelog Into A GitHub Release

The release description may reorganize and explain changelog facts, but it must
cover every material entry. Use this mapping:

| Changelog | GitHub Release |
| --- | --- |
| `Added` | `✨ New` and, for the most important items, `Highlights` |
| `Changed` | `🚀 Improvements` |
| `Deprecated` | `⚠️ Compatibility` |
| `Removed` | `⚠️ Compatibility` |
| `Fixed` | `🐛 Fixes` |
| `Security` | `🔒 Security` |

Use emoji only in section headings as navigation markers. Keep the prose itself
professional and easy to scan.

## GitHub Release Template

Delete empty optional sections, all angle-bracket instructions, and any sentence
that has not been verified. Replace `PREVIOUS` and `X.Y.Z` with real tags.

```markdown
# AIHelper vX.Y.Z

<One or two sentences explaining the release outcome, who benefits, and why the
upgrade matters. Do not repeat the version or say only “This release includes”.>

## Highlights

- <Most important user or operator outcome.>
- <Second important outcome, if material.>
- <Third or fourth outcome only when it helps users choose to upgrade.>

## What’s Changed

### ✨ New

- <Expanded but factual presentation of an Added entry.>

### 🚀 Improvements

- <Expanded but factual presentation of a Changed entry.>

### 🐛 Fixes

- <Expanded but factual presentation of a Fixed entry.>

### 🔒 Security

- <Security impact and required user action, if applicable.>

## ⚠️ Compatibility

- <Breaking change, deprecation, removal, migration step, minimum platform, or
  configuration requirement.>
- <When verified and useful, state that released CLI, JSON/MCP contracts, and
  plugin ABI remain backward compatible.>

## Downloads

| Platform | Archive |
| --- | --- |
| Windows x64 | `ah-windows-x64.zip` |
| Linux x64 | `ah-linux-x64.zip` |
| macOS ARM64 | `ah-macos-arm64.zip` |

Each archive contains the `ah` executable and its executable-relative `plugins/`
directory. Keep them together after extraction.

## Full Changelog

[`vPREVIOUS...vX.Y.Z`](https://github.com/Bobsans/AIHelper/compare/vPREVIOUS...vX.Y.Z)
```

## Composition Rules

- Write the opening summary after the rest of the description. It should
  synthesize the verified release, not introduce new claims.
- Select two to four highlights by user impact. A small patch release may omit
  `Highlights` when the summary and fixes section are sufficient.
- Expand terse changelog bullets only with context established by the release
  diff or validation results.
- Preserve exact command, flag, field, environment-variable, platform, and
  archive names.
- Put breaking changes, removals, deprecations, migration steps, and minimum
  platform changes in `Compatibility`, even when they are also explained in
  `What’s Changed`.
- State backward compatibility only after checking the CLI, stable JSON/MCP
  contracts, and plugin ABI relevant to the release.
- Always include the downloads table for a production binary release and verify
  the actual assets after publication.
- Always include the comparison link, but never use it as the entire release
  description.
- Do not add a contributor list, benchmark claim, security claim, or upgrade
  recommendation unless the underlying evidence is available.

## Pre-Publication Checklist

- The version and date match the release commit and tag.
- Every material changelog entry appears in the GitHub description.
- Every GitHub description claim is supported by the tagged changelog, release
  diff, or validation evidence.
- No empty heading, placeholder, `TBD`, or angle-bracket instruction remains.
- Breaking changes and migration actions are prominent.
- Names of commands, flags, fields, environment variables, platforms, and
  archives are exact.
- The comparison link uses the previous production tag and the new tag.
- The description contains meaningful notes before the comparison link.
- The Markdown preview has a clear hierarchy and readable tables.
- The final published description is checked again through
  `ah github release get vX.Y.Z --json`.
