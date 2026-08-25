# `ah gitlab`

Dynamic plugin domain for GitLab Releases and GitLab CI/CD pipelines.

This domain is provided by external plugin `ah-plugin-gitlab` and is loaded from the `plugins` directory next to `ah`.

GitLab-specific commands intentionally live outside `ah git`; local Git helpers remain provider-neutral.

Interactive structured output uses semantic colors for projects, issue states
and IDs, release tags, pipeline/job states, refs, users, designs, and URLs.
Issue descriptions, comment bodies, and job traces remain unformatted. Colors
are disabled automatically for pipes, redirects, captured output, and JSON.
Set `NO_COLOR` to disable colors explicitly.

## Authentication

The plugin resolves a token in this order:

1. the `token` credential slot, when a vault secret id is selected
2. `--token <TOKEN>`
3. `GITLAB_TOKEN`
4. `GL_TOKEN`
5. `git credential fill` for the `--api-url` host

Public project reads may work without a token. Creating releases and reading private projects require a token with suitable GitLab permissions.

Common flags:

```bash
ah gitlab [--project group/project|PROJECT_ID] [--remote origin] [--host https://gitlab.com] [--api-url https://gitlab.example.com/api/v4] [--graphql-url https://gitlab.example.com/api/graphql] [--token TOKEN] [--use-git-credential[=true|false]] <command>
```

If `--project` is omitted, the plugin tries to parse a GitLab project path from `git remote get-url origin`.
When that remote points at a self-managed instance and `--host` was not given,
the host is taken from the remote instead of failing with
`GITLAB_PROJECT_UNDETECTED`. An explicit `--host` or `--project` is never
overridden this way.

Authentication uses `--token`, then `GITLAB_TOKEN`, then `GL_TOKEN`, then the Git
credential helper.

A vault credential is the preferred source over MCP and is available on every
`gitlab.*` command through the `token` slot:

```bash
ah secrets add work-gitlab --kind gitlab-token --label "Work PAT" --open
```

It works the same way on the direct CLI:

```bash
ah gitlab repo --credential token=work-gitlab
```

Agents pass only the id, `{"credentials": {"token": "work-gitlab"}}`. An inline `token`
argument is rejected over MCP with `INVALID_ARGUMENT`; `--token` remains
available on the CLI. A vault credential and an inline `--token` are mutually
exclusive. Like `--token`, a vault credential reaches whatever https host you
name, since the caller selected both the credential and the destination.

Ambient credentials — the two environment variables and the credential helper —
are host-bound. They are sent only to `gitlab.com`, to the host of the detected
git remote, or to a loopback host. Pointing `--api-url` at any other host silently
drops them, so a redirected endpoint cannot collect your GitLab credentials. The
token is bound to the `--api-url` host at resolution and re-checked on every
request, so a `--graphql-url` on a different host never receives it. Pass
`--use-git-credential=false` to skip the helper entirely.

An explicit `--token` reaches whatever https host you name, since you supplied
both the credential and the destination. That is the escape hatch for a
self-managed host the rules above do not cover — for example when `--project` is
given explicitly, so no remote is detected: `--token "$GITLAB_TOKEN"`.

No token is ever sent to a cleartext `http://` URL unless the host is loopback;
that combination fails with `GITLAB_INSECURE_TOKEN_TARGET`.

Use `--host` for self-managed GitLab installations — needed when `--project` is
given explicitly, since then no remote is read to derive it:

```bash
ah gitlab --host https://gitlab.example.com project
```

Use `--api-url` when the REST API root is not the standard `<host>/api/v4`.

Use `--graphql-url` when the GraphQL endpoint cannot be derived from `--api-url`. Otherwise an API URL ending in `/api/v4` maps to `/api/graphql`; if it does not, the endpoint defaults to `<host>/api/graphql`.

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

## `ah gitlab issue view`

View issue metadata by internal issue id (`iid`).

```bash
ah gitlab issue view <iid> [--full]
```

Use `--full` to include the issue description, labels, timestamps, comments, and issue designs in one response. Comments come from GitLab's issue notes API. Designs are read through GitLab GraphQL; if that query is unavailable on the selected GitLab instance, the command still returns the issue and comments with a warning. Global `--limit` caps fetched comments and designs; `--full` defaults to 100. Responses expose only the documented issue, note, and design fields; unknown upstream fields are ignored.

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

The polling deadline is checked before every follow-up request; the sleep interval is shortened to the remaining timeout when necessary.

## `ah gitlab pipeline jobs`

List jobs for a pipeline.

```bash
ah gitlab pipeline jobs <pipeline-id>
```

## `ah gitlab job trace`

Read or search a job trace.

```bash
ah gitlab job trace <job-id> [--grep TEXT] [--max-body-bytes BYTES] [--limit N]
```

The trace is filtered while read and is capped at `8388608` bytes by default. Override the cap with `--max-body-bytes`; overflow returns `GITLAB_RESPONSE_TOO_LARGE`.

## `ah gitlab job warnings`

Extract warning-like lines from a job trace.

```bash
ah gitlab job warnings <job-id> [--max-body-bytes BYTES] [--limit N]
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
- `gitlab.issue.view`
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
