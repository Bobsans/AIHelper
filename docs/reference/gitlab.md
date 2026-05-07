# `ah gitlab`

Dynamic plugin domain for GitLab Releases and GitLab CI/CD pipelines.

This domain is provided by external plugin `ah-plugin-gitlab` and is loaded from the `plugins` directory next to `ah`.

GitLab-specific commands intentionally live outside `ah git`; local Git helpers remain provider-neutral.

## Authentication

The plugin resolves a token in this order:

1. `--token <TOKEN>`
2. `GITLAB_TOKEN`
3. `GL_TOKEN`
4. `git credential fill` for the selected host, only when `--use-git-credential` is set

Public project reads may work without a token. Creating releases and reading private projects require a token with suitable GitLab permissions.

Common flags:

```bash
ah gitlab [--project group/project|PROJECT_ID] [--remote origin] [--host https://gitlab.com] [--api-url https://gitlab.example.com/api/v4] [--token TOKEN] [--use-git-credential] <command>
```

If `--project` is omitted, the plugin tries to parse a GitLab project path from `git remote get-url origin`.

Use `--host` for self-managed GitLab installations:

```bash
ah gitlab --host https://gitlab.example.com project
```

Use `--api-url` when the REST API root is not the standard `<host>/api/v4`.

## `ah gitlab project`

Inspect detected project context.

```bash
ah gitlab project
ah --json gitlab --project group/tool --host https://gitlab.example.com project
```

## `ah gitlab releases`

List releases.

```bash
ah gitlab releases [--limit N]
```

## `ah gitlab release get`

Get release metadata by tag.

```bash
ah gitlab release get <tag>
```

## `ah gitlab release create`

Create a GitLab Release for a tag.

```bash
ah gitlab release create <tag> [--name NAME] [--description TEXT|--description-file PATH] [--ref REF]
```

This command does not bump versions, edit changelogs, commit, tag, or push. It only calls the GitLab Releases API.

## `ah gitlab issues`

List project issues.

```bash
ah gitlab issues [--state opened|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT] [--limit N]
```

`--since` maps to GitLab's `updated_after` filter. Custom GitLab hosts keep using the same global `--host` and `--api-url` options as the rest of the plugin.

## `ah gitlab issue get`

Get issue metadata by internal issue id (`iid`).

```bash
ah gitlab issue get <iid>
```

## `ah gitlab issue create`

Create an issue.

```bash
ah gitlab issue create --title TITLE [--description TEXT|--description-file PATH] [--label LABEL ...] [--assignee-id ID ...]
```

## `ah gitlab issue update`

Update issue fields.

```bash
ah gitlab issue update <iid> [--title TITLE] [--description TEXT|--description-file PATH] [--state opened|closed] [--label LABEL ...] [--assignee-id ID ...]
```

## `ah gitlab issue close`

Close an issue, optionally after adding a comment.

```bash
ah gitlab issue close <iid> [--comment TEXT|--comment-file PATH]
```

## `ah gitlab issue comment`

Add an issue comment.

```bash
ah gitlab issue comment <iid> --body TEXT|--body-file PATH
```

## `ah gitlab issue comments`

List issue comments.

```bash
ah gitlab issue comments <iid> [--limit N]
```

## `ah gitlab pipelines`

List pipelines.

```bash
ah gitlab pipelines [--branch BRANCH] [--limit N]
```

`--branch` maps to GitLab's pipeline `ref` filter.

## `ah gitlab pipeline get`

Get pipeline metadata.

```bash
ah gitlab pipeline get <pipeline-id>
```

## `ah gitlab pipeline wait`

Wait for a pipeline to reach a terminal status.

```bash
ah gitlab pipeline wait <pipeline-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]
```

Terminal statuses are `success`, `failed`, `canceled`, `skipped`, and `manual`.

## `ah gitlab pipeline jobs`

List jobs for a pipeline.

```bash
ah gitlab pipeline jobs <pipeline-id>
```

## `ah gitlab job trace`

Read or search a job trace.

```bash
ah gitlab job trace <job-id> [--grep TEXT] [--limit N]
```

## `ah gitlab job warnings`

Extract warning-like lines from a job trace.

```bash
ah gitlab job warnings <job-id> [--limit N]
```

The warning matcher is intentionally broad for AI-agent triage. It matches lines containing terms such as `warning`, `deprecated`, `deprecation`, and `will be removed`.

## Output

Text output is compact by default. Use global `--json` for structured machine-readable output.

Stable command identifiers in JSON include:

- `gitlab.project`
- `gitlab.releases`
- `gitlab.release.get`
- `gitlab.release.create`
- `gitlab.issues`
- `gitlab.issue.get`
- `gitlab.issue.create`
- `gitlab.issue.update`
- `gitlab.issue.close`
- `gitlab.issue.comment`
- `gitlab.issue.comments`
- `gitlab.pipelines`
- `gitlab.pipeline.get`
- `gitlab.pipeline.wait`
- `gitlab.pipeline.jobs`
- `gitlab.job.trace`
- `gitlab.job.warnings`
