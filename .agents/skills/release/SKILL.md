---
name: release
description: Release, prepare release, publish release, or выпустить релиз for AIHelper. Use when inspecting, versioning, validating, publishing, or verifying an AIHelper GitHub binary release.
---

# AIHelper Release

Orchestrate the repository's production binary release as a sequence of guarded
phases. Do not publish workspace crates to crates.io.

## Load The Release Contract

Before acting, read these files as the sources of truth:

- `AGENTS.md`
- `docs/agents/recipes/release.md`
- `docs/agents/recipes/release-notes.md`
- `CHANGELOG.md`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`

Use `ah ai info` before unfamiliar work. Prefer `ah` commands for repository,
Git, project, and checked-command operations. Consult durable release history in
the `aihelper` basic-memory project when it is available. Report unavailable
context instead of inventing it.

The recipes own command details, artifact names, release-note structure, and
completion criteria. This skill owns phase selection and safety gates. If they
conflict, stop and surface the conflict before mutating state.

## Establish Authority

Classify the user's request before taking action:

| Mode    | Sufficient request                                           | Allowed work                                                                      |
|---------|--------------------------------------------------------------|-----------------------------------------------------------------------------------|
| Inspect | Inspect, plan, recommend a version, or explain release state | Read-only inspection and recommendations                                          |
| Prepare | Prepare a release, bump a version, or draft release notes    | Local version, `Cargo.lock`, changelog, notes, and validation changes             |
| Publish | Explicitly publish or complete the release                   | The guarded commit, push, preflight, tag, GitHub Release, and verification phases |

A generic request such as "do a release" is not publication authority. Ask one
short question to choose `Prepare` or `Publish` before crossing that boundary.
Preparation authority never implies permission to commit, push, dispatch a
workflow, create or push a tag, or create a GitHub Release.

Never replace, move, delete, recreate, or overwrite an existing tag or GitHub
Release without separate explicit approval. Never silently enable the `github`
plugin. If a required domain is disabled, report the blocker and ask before
changing the plugin state.

## Preserve Repository State

At the start of every mode, inspect the branch, upstream, working tree, latest
tag, remotes, and all workspace package versions using the commands in the
release recipe.

- Inspect every staged, unstaged, and untracked path before editing or
  committing.
- Treat concurrent changes as authoritative user changes. Preserve them and
  adapt the release scope around them.
- Do not include a change in the release commit until its purpose and ownership
  are understood.
- Stop and ask when a dirty tree makes the intended release contents ambiguous.
- Do not use destructive Git commands or bypass hooks and checks.

State the selected mode, proposed version, target branch, and target commit when
known. Do not claim a clean or synchronized state without checking it.

## Inspect And Select The Version

Compare changes since the latest production tag against the released public
surface. Recommend Semantic Versioning from compatibility impact:

- `PATCH` preserves CLI behavior, stable JSON and MCP contracts, and plugin ABI.
- `MINOR` adds backward-compatible commands, tools, fields, or plugin behavior.
- `MAJOR` makes a released CLI, JSON/MCP contract, plugin ABI, or C ABI change
  incompatible.

Use a prerelease only when explicitly requested. Report a requested version that
conflicts with the observed SemVer category before editing. Use
`ah project version --json` to discover every workspace package rather than
assuming a fixed package list, and require one release version across all of
them.

Inspection mode ends after reporting the evidence, recommendation, repository
state, blockers, and the next authorized phase.

## Prepare

In preparation or publication mode:

1. Update every detected workspace package manifest to the selected version.
2. Synchronize only corresponding local workspace package entries in
   `Cargo.lock`.
3. Move released `[Unreleased]` entries into `## [X.Y.Z] - YYYY-MM-DD`, leaving
   `[Unreleased]` present.
4. Draft GitHub notes from the same verified facts by following
   `release-notes.md`.
5. Search manifests and local lockfile package entries for stale workspace
   versions, then confirm `ah project version --json` reports one version.
6. Inspect the complete release diff. Do not add features or unrelated
   refactors while preparing a release.

Ground every changelog and release-note claim in the release diff, tagged
changelog, completed checks, or verified publication metadata. Remove empty
sections and placeholders. Do not infer performance, security, compatibility,
or migration claims.

## Validate Locally

Run every required check through AIHelper:

```text
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
ah run check cargo build --release --locked
```

Do not commit, tag, or publish after a failed required check. If the host cannot
run a check, report the limitation and require equivalent successful CI evidence
before publication. Preparation mode stops after reporting the diff and results;
leave commit and external state untouched.

## Create The Release Commit

Continue only in publication mode after local validation succeeds.

1. Reinspect every changed path and confirm that only intended release content
   will be committed.
2. Confirm the release branch is `main` and identify the exact release target.
3. Create the focused commit `chore: release vX.Y.Z` without amending or bypassing
   hooks.
4. Push `main` normally and confirm local `main` and `origin/main` resolve to the
   exact release commit.

Stop if the push fails, the branch diverges, the tree changes unexpectedly, or
the commit identity cannot be proven.

## Run Cross-Platform Preflight

Before creating a tag or GitHub Release, dispatch `release.yml` on `main` as
documented in the release recipe. Match the workflow run by event, dispatch
time, and exact release commit SHA rather than selecting the first returned run.

Wait for completion and inspect all jobs, warnings, and artifacts. Require
`ah-linux-x64.zip`, `ah-windows-x64.zip`, and `ah-macos-arm64.zip`. Every matrix
job and packaged CLI/MCP smoke test is blocking. Missing or failed output ends
the publication attempt before tagging.

## Publish

After preflight succeeds:

1. Reconfirm the clean working tree, release commit, synchronized `main`, and
   release notes.
2. Prove that the exact version tag does not exist locally or remotely and that
   no GitHub Release exists for it. Stop if either exists.
3. Create the annotated `vX.Y.Z` tag on the exact release commit and verify its
   target before pushing it.
4. Push only that tag to `origin`.
5. Create the non-draft, non-prerelease GitHub Release with title `vX.Y.Z` and
   the verified notes file.

If tag creation or push fails, do not create the GitHub Release. If Release
creation fails after the tag was pushed, preserve the public state and report
the exact partial result; do not delete or recreate references automatically.

## Verify Publication

Find the release-triggered workflow run by event and tagged commit, then wait for
it to complete. Verify all criteria from the release recipe, including:

- `main`, `origin/main`, and the annotated tag identify the release commit;
- the Release is published with `draft=false` and `prerelease=false`;
- its description is complete, accurate, and supported by evidence;
- all required ZIP assets are attached;
- the release workflow completed successfully;
- the final repository state is clean and synchronized.

For runtime, plugin, packaging, or MCP changes, smoke-test an installed archive
as required by the recipe. Verify the executable-relative `plugins/` directory;
for MCP-sensitive changes, also verify the stdio handshake, `tools/list`, risk
metadata, and a representative typed tool call.

Record the verified milestone in the `aihelper` basic-memory project with the
version, URL, release commit, tag target, workflow run IDs, assets, checks,
limitations, smoke results, final repository state, and any deviation.

## Handle Failure And Report State

On any failure, preserve evidence and stop before the next irreversible phase.
Do not repair the published state, rerun an operation that may overwrite it, or
weaken a gate without explicit approval.

Every phase report must distinguish:

- completed actions and their exact identifiers;
- state that was inspected but not changed;
- remaining phases that were not authorized or did not run;
- blockers, failures, host limitations, and partial external state;
- the next action that requires user authority.
