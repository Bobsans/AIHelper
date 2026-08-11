# Recipe: Prepare And Publish A Release

## Goal

Prepare, validate, publish, and verify a production AIHelper GitHub Release
without changing public contracts accidentally or overwriting existing release
state.

The release artifact is the cross-platform GitHub binary distribution. This
process does not publish workspace crates to crates.io.

## Authority Boundary

Treat release preparation and release publication as separate phases.

- Read-only inspection and version recommendations do not require publication
  authority.
- Editing versions, `Cargo.lock`, and `CHANGELOG.md` is local release
  preparation.
- Committing, pushing, dispatching GitHub Actions, creating or pushing a tag,
  and creating a GitHub Release mutate persistent local or external state.
- Perform external mutations only when the user explicitly asks to publish or
  complete the release. A request to inspect, plan, or prepare a release does
  not authorize publication.
- Never replace, move, delete, or recreate an existing tag or GitHub Release
  without separate explicit approval.

## Establish Release Context

Start with the project manual, durable release history, and repository state:

```bash
ah ai info
ah git status --json
ah git changed --json
ah git tags --latest --json
ah git remotes --json
ah project version --json
```

Also read:

- `AGENTS.md`
- `CHANGELOG.md`
- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- recent release records from the `aihelper` basic-memory project

Before editing:

1. Confirm the current branch, upstream, latest release tag, and release target
   commit.
2. Inspect every staged, unstaged, and untracked path.
3. Treat changes that appear while work is in progress as authoritative user
   changes. Preserve them and adapt the release scope around them.
4. Do not create a release commit from a dirty tree until every included change
   is understood and intended for the release.
5. Confirm `main` is synchronized with `origin/main` before publication.

## Select The Version

Use Semantic Versioning and judge compatibility against the released public
surface, not against implementation size.

| Increment | Use when |
| --- | --- |
| `PATCH` | Fixes, performance improvements, documentation, or internal changes preserve CLI behavior, stable JSON/MCP contracts, and plugin ABI compatibility. |
| `MINOR` | Backward-compatible capabilities are added, such as commands, tools, fields, or plugin functionality. |
| `MAJOR` | A released command or behavior is removed or incompatibly changed, a stable JSON field name/type changes, an MCP contract breaks, or the plugin/C ABI becomes incompatible. |

Use a prerelease only when the user explicitly requests one. If the requested
version conflicts with the SemVer category, report the conflict before editing.

All detected Cargo workspace packages must have the same release version. The
workspace currently includes `aihelper`, `ah-mcp`, `ah-plugin-api`, `ah-runtime`,
and the GitHub, GitLab, Ollama, and PostgreSQL plugins. Use
`ah project version --json` as the authoritative completeness check rather than
assuming the package count will remain fixed.

## Prepare The Release Change

1. Update the version in every workspace package manifest.
2. Synchronize only the corresponding local package entries in `Cargo.lock`.
3. Move the accumulated `CHANGELOG.md` entries from `[Unreleased]` into
   `## [X.Y.Z] - YYYY-MM-DD`.
4. Keep `[Unreleased]` present and empty unless unreleased work intentionally
   remains outside the release.
5. Write the changelog section and GitHub Release body using
   [`release-notes.md`](release-notes.md). Keep the changelog concise and make
   the release body editorially useful without adding unsupported claims.
6. Treat the tagged changelog, verified release diff, and validation results as
   the factual source for every release-note statement. A comparison link may
   supplement the description, but never replace it.
7. Verify that no old workspace version remains in manifests or local package
   entries and that `ah project version --json` reports one version everywhere.

Do not add unrelated functionality or refactor production code during release
preparation. Fix only blockers required to make the intended release valid.

## Validate Locally

Run the repository checks through AIHelper:

```bash
ah run check cargo fmt --all -- --check
ah run check cargo test --workspace --all-targets --locked
ah run check cargo build --locked
ah run check cargo build --release --locked
```

Do not tag or publish if any required check fails. If a check cannot run because
of the host environment, report the limitation and require equivalent CI
evidence before publication.

Inspect the final release diff. It should contain the intended product changes
plus focused version, lockfile, changelog, test, and documentation updates. The
release-specific commit convention is:

```text
chore: release vX.Y.Z
```

## Run The Cross-Platform Preflight

After the validated release commit is on `origin/main`, dispatch the release
workflow without publishing a GitHub Release:

```bash
ah github workflow run release.yml --ref main
ah github runs --workflow release.yml --json
ah github run wait <run-id> --fail-on-failure
ah github run jobs <run-id> --json
ah github run artifacts <run-id> --json
ah github run warnings <run-id>
```

Match the run by event, target commit SHA, and dispatch time; do not assume the
first returned run is the new preflight.

The preflight must build and package:

- `ah-linux-x64.zip`
- `ah-windows-x64.zip`
- `ah-macos-arm64.zip`

Inspect individual jobs and artifacts. All matrix jobs are blocking and each
archive must pass packaged CLI and MCP smoke checks before upload. AIHelper
release policy requires all three expected archives before publication. The
Windows archive must also contain `ah-mcp-service.exe` and
`ah-update-helper.exe`; the update helper's side-effect-free `--self-check` must
report helper protocol v1 with the same version and target as the packaged
`ah.exe`.

## Publish

After the preflight succeeds:

1. Confirm the release commit hash and verify the working tree has not changed.
2. Create annotated tag `vX.Y.Z` on that exact commit.
3. Verify the local tag resolves to the intended commit.
4. Push the tag to `origin`.
5. Create GitHub Release `vX.Y.Z` with title `vX.Y.Z`, the changelog-derived
   notes, `draft=false`, and `prerelease=false`.

Representative commands:

```bash
ah git commit-info <release-commit> --json
ah git tag create vX.Y.Z --ref <release-commit> --message "AIHelper vX.Y.Z"
git push origin vX.Y.Z
ah github release create vX.Y.Z --title vX.Y.Z --notes-file <notes-file>
```

The published Release triggers `.github/workflows/release.yml` again. On a
release event, its publish job downloads the platform artifacts and attaches
the ZIP files to the GitHub Release.

## Verify The Published Release

Locate the release-triggered run by event and tagged commit, wait for it, then
inspect failures, warnings, jobs, and artifacts as needed:

```bash
ah github runs --workflow release.yml --json
ah github run wait <run-id> --fail-on-failure
ah github release get vX.Y.Z --json
ah github release assets vX.Y.Z --json
```

Completion requires all of the following:

- `main` and `origin/main` identify the validated release commit.
- The annotated tag identifies that same commit.
- The GitHub Release is published with `draft=false` and `prerelease=false`.
- Its description covers the tagged changelog section accurately and contains
  no claims that are unsupported by the release diff or validation evidence.
- Linux x64, Windows x64, and macOS ARM64 archive, manifest, and signature
  triplets are attached: exactly nine release assets.
- The release workflow has been inspected through completion.
- The final repository state is clean and synchronized.

For runtime, plugin, packaging, or MCP changes, smoke-test an installed archive:

```bash
ah --version
ah plugins list --json
```

Verify the archive contains the executable and the executable-relative
`plugins/` directory. For Windows, also verify `ah-mcp-service.exe` is present,
then run `ah-update-helper.exe --self-check` and verify its strict protocol
identity.
For MCP-sensitive changes, also perform a stdio handshake, inspect `tools/list`,
confirm risk metadata, and execute a representative typed tool call.

For updater-sensitive releases, use a supported Windows VM with the previous
signed release installed. Run `ah upgrade --check --json`, apply the published
release with `ah upgrade --json`, verify the installed version and any previously
ready managed MCP, then run `ah upgrade --rollback --json`. Record interruption
and reboot results separately; local tests do not replace this acceptance.

## Failure Handling

- Local validation failure: stop before tagging or publication and preserve the
  evidence needed to diagnose the failure.
- Missing preflight platform archive: inspect jobs and logs, fix the cause, and
  repeat the preflight before publishing.
- Tag creation or push failure: stop before creating the GitHub Release and
  report which local and remote references exist.
- Existing tag or Release: stop; do not overwrite it.
- Published workflow failure: keep the public state intact, inspect jobs,
  warnings, and logs, and request approval before corrective external changes.
- Incorrect release notes or assets: report the exact mismatch and obtain
  approval before updating or rerunning any operation that can overwrite public
  release state.

## Record The Milestone

After verification, write a durable release note to the `aihelper` basic-memory
project. Record at least:

- version, release URL, release commit, and annotated tag target;
- production and preflight workflow run IDs and conclusions;
- published asset names;
- checks performed and any host limitations;
- smoke-test results;
- final branch, upstream, tag, and working-tree state;
- any correction or deviation from this recipe.
