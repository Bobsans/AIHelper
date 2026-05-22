# `ah github`

Dynamic plugin domain for GitHub Releases and GitHub Actions.

This domain is provided by external plugin `ah-plugin-github` and is loaded from `plugins` directory next to `ah`.

GitHub-specific commands intentionally live outside `ah git`; local Git helpers remain provider-neutral.

## Authentication

The plugin resolves a token in this order:

1. `--token <TOKEN>`
2. `GITHUB_TOKEN`
3. `GH_TOKEN`
4. `git credential fill` for `github.com`, only when `--use-git-credential` is set

Public repository reads may work without a token. Creating releases, dispatching workflows, and reading private repositories require a token with suitable GitHub permissions.

Common flags:

```bash
ah github [--repo OWNER/REPO] [--remote origin] [--api-url https://api.github.com] [--token TOKEN] [--use-git-credential] <command>
```

If `--repo` is omitted, the plugin tries to parse `owner/repo` from `git remote get-url origin`.

## `ah github repo`

Inspect detected repository context.

```bash
ah github repo
ah --json github --repo Bobsans/AIHelper repo
```

## `ah github issues`

List repository issues. Pull requests are filtered out of the returned issue list.

```bash
ah github issues [--state open|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT] [--limit N]
```

`--search` uses GitHub issue search scoped to the selected repository.

## `ah github issue view`

View issue metadata by issue number.

```bash
ah github issue view <number>
```

## `ah github issue create`

Create an issue.

```bash
ah github issue create --title TITLE [--body TEXT|--body-file PATH] [--label LABEL ...] [--assignee USER ...]
```

## `ah github issue update`

Update issue fields.

```bash
ah github issue update <number> [--title TITLE] [--body TEXT|--body-file PATH] [--state open|closed] [--label LABEL ...] [--assignee USER ...]
```

## `ah github issue close`

Close an issue, optionally after adding a comment.

```bash
ah github issue close <number> [--comment TEXT|--comment-file PATH]
```

## `ah github issue comment`

Add an issue comment.

```bash
ah github issue comment <number> --body TEXT|--body-file PATH
```

## `ah github issue comments`

List issue comments.

```bash
ah github issue comments <number> [--limit N]
```

## `ah github release get`

Get release metadata by tag.

```bash
ah github release get <tag>
```

Example:

```bash
ah github release get v0.3.0
```

## `ah github release assets`

List release assets by tag.

```bash
ah github release assets <tag>
```

## `ah github release create`

Create a GitHub Release for a tag.

```bash
ah github release create <tag> [--title TITLE] [--notes TEXT|--notes-file PATH] [--target REF] [--draft] [--prerelease]
```

This command does not bump versions, edit changelogs, commit, tag, or push. It only calls the GitHub Release API.

## `ah github workflows`

List GitHub Actions workflows.

```bash
ah github workflows
```

## `ah github workflow run`

Dispatch a workflow by id or file name.

```bash
ah github workflow run <workflow> --ref <ref> [--input KEY=VALUE ...]
```

Example:

```bash
ah github workflow run release.yml --ref main
```

## `ah github runs`

List workflow runs.

```bash
ah github runs [--workflow WORKFLOW] [--branch BRANCH] [--limit N]
```

Example:

```bash
ah github runs --workflow release.yml --branch main --limit 5
```

## `ah github run get`

Get workflow run metadata.

```bash
ah github run get <run-id>
```

## `ah github run wait`

Wait for a workflow run to complete.

```bash
ah github run wait <run-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]
```

## `ah github run jobs`

List jobs for a workflow run.

```bash
ah github run jobs <run-id>
```

## `ah github run logs`

Download workflow run logs and search them.

```bash
ah github run logs <run-id> [--grep TEXT] [--limit N]
```

GitHub exposes run logs as a zip archive; the plugin reads text files from the archive and emits matching lines.

## `ah github run warnings`

Extract warning-like lines from workflow run logs.

```bash
ah github run warnings <run-id> [--limit N]
```

The warning matcher is intentionally broad for AI-agent triage. It matches lines containing terms such as `warning`, `deprecated`, `deprecation`, and `will be removed`.

## `ah github run artifacts`

List artifacts for a workflow run.

```bash
ah github run artifacts <run-id>
```

## Output

Text output is compact by default. Use global `--json` for structured machine-readable output.

Stable command identifiers in JSON include:

- `github.repo`
- `github.issues`
- `github.issue.view`
- `github.issue.create`
- `github.issue.update`
- `github.issue.close`
- `github.issue.comment`
- `github.issue.comments`
- `github.release.get`
- `github.release.assets`
- `github.release.create`
- `github.workflows`
- `github.workflow.run`
- `github.runs`
- `github.run.get`
- `github.run.wait`
- `github.run.jobs`
- `github.run.logs`
- `github.run.warnings`
- `github.run.artifacts`
