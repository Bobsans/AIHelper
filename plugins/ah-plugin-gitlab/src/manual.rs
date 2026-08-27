//! What `ah gitlab --help` and the plugin manual show.
//!
//! Prose and examples, kept apart from the code they describe because a
//! reworded example is not a behaviour change. Every example here is parsed by
//! a test, so a stale one fails the build.

use super::*;

pub(crate) fn plugin_manual() -> PluginManual {
    PluginManual {
        plugin_name: PLUGIN_NAME.to_owned(),
        domain: DOMAIN.to_owned(),
        description: DESCRIPTION.to_owned(),
        commands: vec![
            ManualCommand {
                name: "project".to_owned(),
                summary: "Detect GitLab project context.".to_owned(),
                usage: "project [--project PATH_OR_ID] [--remote NAME] [--host URL] [--api-url URL] [--graphql-url URL] [--token TOKEN] [--use-git-credential[=true|false]]".to_owned(),
                examples: vec![ManualExample::new("Inspect current GitLab project", &["project"])],
            },
            ManualCommand {
                name: "releases".to_owned(),
                summary: "List GitLab releases.".to_owned(),
                usage: "releases [--project PATH_OR_ID]".to_owned(),
                examples: vec![ManualExample::new("List releases", &["releases"])],
            },
            ManualCommand {
                name: "release get".to_owned(),
                summary: "Get release metadata by tag.".to_owned(),
                usage: "release get <tag> [--project PATH_OR_ID]".to_owned(),
                examples: vec![ManualExample::new("Inspect release v1.0.0", &["release", "get", "v1.0.0"])],
            },
            ManualCommand {
                name: "release create".to_owned(),
                summary: "Create a GitLab release for a tag.".to_owned(),
                usage: "release create <tag> [--name NAME] [--description TEXT|--description-file PATH] [--ref REF]".to_owned(),
                examples: vec![ManualExample::new(
                    "Create release from description file",
                    &["release", "create", "v1.0.1", "--name", "v1.0.1", "--description-file", "RELEASE_NOTES.md"],
                )],
            },
            ManualCommand {
                name: "issues".to_owned(),
                summary: "List GitLab issues.".to_owned(),
                usage: "issues [--state opened|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT]".to_owned(),
                examples: vec![ManualExample::new("List open bugs", &["issues", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue view".to_owned(),
                summary: "View issue metadata, optionally with comments and designs.".to_owned(),
                usage: "issue view <iid> [--full]".to_owned(),
                examples: vec![ManualExample::new("Inspect issue", &["issue", "view", "42"])],
            },
            ManualCommand {
                name: "issue create".to_owned(),
                summary: "Create an issue.".to_owned(),
                usage: "issue create --title TITLE [--description TEXT|--description-file PATH] [--label LABEL ...] [--assignee-id ID ...]".to_owned(),
                examples: vec![ManualExample::new("Create bug issue", &["issue", "create", "--title", "Fix build", "--description", "Build fails", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue update".to_owned(),
                summary: "Update issue fields.".to_owned(),
                usage: "issue update <iid> [--title TITLE] [--description TEXT|--description-file PATH] [--state opened|closed] [--label LABEL ...] [--assignee-id ID ...]".to_owned(),
                examples: vec![ManualExample::new("Close issue via update", &["issue", "update", "42", "--state", "closed"])],
            },
            ManualCommand {
                name: "issue close".to_owned(),
                summary: "Close an issue, optionally after adding a comment.".to_owned(),
                usage: "issue close <iid> [--comment TEXT|--comment-file PATH]".to_owned(),
                examples: vec![ManualExample::new("Close with comment", &["issue", "close", "42", "--comment", "Fixed in main"])],
            },
            ManualCommand {
                name: "issue comment".to_owned(),
                summary: "Add an issue comment.".to_owned(),
                usage: "issue comment <iid> --body TEXT|--body-file PATH".to_owned(),
                examples: vec![ManualExample::new("Comment on issue", &["issue", "comment", "42", "--body", "I can reproduce this"])],
            },
            ManualCommand {
                name: "issue comments".to_owned(),
                summary: "List issue comments.".to_owned(),
                usage: "issue comments <iid>".to_owned(),
                examples: vec![ManualExample::new("List comments", &["issue", "comments", "42"])],
            },
            ManualCommand {
                name: "pipelines".to_owned(),
                summary: "List GitLab pipelines.".to_owned(),
                usage: "pipelines [--branch BRANCH]".to_owned(),
                examples: vec![ManualExample::new("List main pipelines", &["pipelines", "--branch", "main"])],
            },
            ManualCommand {
                name: "pipeline get".to_owned(),
                summary: "Get pipeline metadata.".to_owned(),
                usage: "pipeline get <pipeline-id>".to_owned(),
                examples: vec![ManualExample::new("Inspect one pipeline", &["pipeline", "get", "42"])],
            },
            ManualCommand {
                name: "pipeline wait".to_owned(),
                summary: "Wait for pipeline completion.".to_owned(),
                usage: "pipeline wait <pipeline-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]".to_owned(),
                examples: vec![ManualExample::new("Wait for one pipeline", &["pipeline", "wait", "42", "--fail-on-failure"])],
            },
            ManualCommand {
                name: "pipeline jobs".to_owned(),
                summary: "List jobs for a pipeline.".to_owned(),
                usage: "pipeline jobs <pipeline-id>".to_owned(),
                examples: vec![ManualExample::new("Inspect pipeline jobs", &["pipeline", "jobs", "42"])],
            },
            ManualCommand {
                name: "job trace".to_owned(),
                summary: "Read or search a job trace.".to_owned(),
                usage: "job trace <job-id> [--grep TEXT] [--max-body-bytes BYTES]".to_owned(),
                examples: vec![ManualExample::new("Search job trace", &["job", "trace", "7", "--grep", "warning"])],
            },
            ManualCommand {
                name: "job warnings".to_owned(),
                summary: "Extract warning-like lines from a job trace.".to_owned(),
                usage: "job warnings <job-id> [--max-body-bytes BYTES]".to_owned(),
                examples: vec![ManualExample::new("List job warnings", &["job", "warnings", "7"])],
            },
        ],
        notes: vec![
            "GitLab-specific features live in this dynamic plugin; local Git commands stay in `ah git`.".to_owned(),
            "Project defaults to a GitLab path parsed from `origin`; override with --project group/project or numeric id.".to_owned(),
            "Use --host for self-managed GitLab, --api-url for nonstandard REST roots, and --graphql-url for a separately configured GraphQL endpoint.".to_owned(),
            "Authentication checks --token, GITLAB_TOKEN, GL_TOKEN, then the Git credential helper; pass --use-git-credential=false to skip the helper.".to_owned(),
            "GITLAB_TOKEN, GL_TOKEN and the credential helper reach only gitlab.com, the detected remote host, or loopback; any other --api-url needs an explicit --token.".to_owned(),
            "Tokens are never sent to a cleartext --api-url unless the host is loopback, and a --graphql-url on another host is never given the token.".to_owned(),
            "Use global --json for stable machine-readable output and --limit to cap releases, pipelines, or trace matches.".to_owned(),
            "Job traces default to an 8 MiB response budget; override with --max-body-bytes.".to_owned(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn manual_examples_parse() {
        let manual = plugin_manual();
        for command in &manual.commands {
            for example in &command.examples {
                let mut args = Vec::with_capacity(example.argv.len() + 1);
                args.push(manual.domain.clone());
                args.extend(example.argv.iter().cloned());
                let parse_result = GitlabCli::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for command '{}': argv={args:?}",
                    command.name
                );
            }
        }
    }
}
