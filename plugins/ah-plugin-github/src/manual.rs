//! What `ah github --help` and the plugin manual show.
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
                name: "repo".to_owned(),
                summary: "Detect GitHub repository context.".to_owned(),
                usage: "repo [--repo OWNER/REPO] [--remote NAME] [--api-url URL] [--token TOKEN] [--use-git-credential[=true|false]]".to_owned(),
                examples: vec![ManualExample::new("Inspect current GitHub repository", &["repo"])],
            },
            ManualCommand {
                name: "issues".to_owned(),
                summary: "List GitHub issues.".to_owned(),
                usage: "issues [--state open|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT]".to_owned(),
                examples: vec![ManualExample::new("List open bugs", &["issues", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue view".to_owned(),
                summary: "View issue metadata.".to_owned(),
                usage: "issue view <number>".to_owned(),
                examples: vec![ManualExample::new("Inspect issue", &["issue", "view", "42"])],
            },
            ManualCommand {
                name: "issue create".to_owned(),
                summary: "Create an issue.".to_owned(),
                usage: "issue create --title TITLE [--body TEXT|--body-file PATH] [--label LABEL ...] [--assignee USER ...]".to_owned(),
                examples: vec![ManualExample::new("Create bug issue", &["issue", "create", "--title", "Fix build", "--body", "Build fails", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue update".to_owned(),
                summary: "Update issue fields.".to_owned(),
                usage: "issue update <number> [--title TITLE] [--body TEXT|--body-file PATH] [--state open|closed] [--label LABEL ...] [--assignee USER ...]".to_owned(),
                examples: vec![ManualExample::new("Close issue via update", &["issue", "update", "42", "--state", "closed"])],
            },
            ManualCommand {
                name: "issue close".to_owned(),
                summary: "Close an issue, optionally after adding a comment.".to_owned(),
                usage: "issue close <number> [--comment TEXT|--comment-file PATH]".to_owned(),
                examples: vec![ManualExample::new("Close with comment", &["issue", "close", "42", "--comment", "Fixed in main"])],
            },
            ManualCommand {
                name: "issue comment".to_owned(),
                summary: "Add an issue comment.".to_owned(),
                usage: "issue comment <number> --body TEXT|--body-file PATH".to_owned(),
                examples: vec![ManualExample::new("Comment on issue", &["issue", "comment", "42", "--body", "I can reproduce this"])],
            },
            ManualCommand {
                name: "issue comments".to_owned(),
                summary: "List issue comments.".to_owned(),
                usage: "issue comments <number>".to_owned(),
                examples: vec![ManualExample::new("List comments", &["issue", "comments", "42"])],
            },
            ManualCommand {
                name: "release get".to_owned(),
                summary: "Get release metadata by tag.".to_owned(),
                usage: "release get <tag> [--repo OWNER/REPO]".to_owned(),
                examples: vec![ManualExample::new("Inspect release v0.3.0", &["release", "get", "v0.3.0"])],
            },
            ManualCommand {
                name: "release assets".to_owned(),
                summary: "List release assets by tag.".to_owned(),
                usage: "release assets <tag> [--repo OWNER/REPO]".to_owned(),
                examples: vec![ManualExample::new("List release assets", &["release", "assets", "v0.3.0"])],
            },
            ManualCommand {
                name: "release create".to_owned(),
                summary: "Create a GitHub Release for a tag.".to_owned(),
                usage: "release create <tag> [--title TITLE] [--notes TEXT|--notes-file PATH] [--target REF] [--draft] [--prerelease]".to_owned(),
                examples: vec![ManualExample::new(
                    "Create release from notes file",
                    &["release", "create", "v0.3.1", "--title", "v0.3.1", "--notes-file", "RELEASE_NOTES.md"],
                )],
            },
            ManualCommand {
                name: "workflows".to_owned(),
                summary: "List GitHub Actions workflows.".to_owned(),
                usage: "workflows [--repo OWNER/REPO]".to_owned(),
                examples: vec![ManualExample::new("List workflows", &["workflows"])],
            },
            ManualCommand {
                name: "workflow run".to_owned(),
                summary: "Dispatch a workflow by id or file name.".to_owned(),
                usage: "workflow run <workflow> --ref <ref> [--input KEY=VALUE ...]".to_owned(),
                examples: vec![ManualExample::new(
                    "Run release workflow on main",
                    &["workflow", "run", "release.yml", "--ref", "main"],
                )],
            },
            ManualCommand {
                name: "runs".to_owned(),
                summary: "List workflow runs.".to_owned(),
                usage: "runs [--workflow WORKFLOW] [--branch BRANCH]".to_owned(),
                examples: vec![ManualExample::new(
                    "List release workflow runs",
                    &["runs", "--workflow", "release.yml", "--branch", "main"],
                )],
            },
            ManualCommand {
                name: "run get".to_owned(),
                summary: "Get workflow run metadata.".to_owned(),
                usage: "run get <run-id>".to_owned(),
                examples: vec![ManualExample::new("Inspect one run", &["run", "get", "25451983278"])],
            },
            ManualCommand {
                name: "run wait".to_owned(),
                summary: "Wait for workflow run completion.".to_owned(),
                usage: "run wait <run-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]".to_owned(),
                examples: vec![ManualExample::new("Wait for one run", &["run", "wait", "25451983278", "--fail-on-failure"])],
            },
            ManualCommand {
                name: "run jobs".to_owned(),
                summary: "List jobs for a workflow run.".to_owned(),
                usage: "run jobs <run-id>".to_owned(),
                examples: vec![ManualExample::new("Inspect run jobs", &["run", "jobs", "25451983278"])],
            },
            ManualCommand {
                name: "run logs".to_owned(),
                summary: "Search workflow run logs.".to_owned(),
                usage: "run logs <run-id> [--grep TEXT] [--max-body-bytes BYTES] [--max-expanded-bytes BYTES]".to_owned(),
                examples: vec![ManualExample::new(
                    "Search logs for Node warning",
                    &["run", "logs", "25451983278", "--grep", "Node.js 20 actions are deprecated"],
                )],
            },
            ManualCommand {
                name: "run warnings".to_owned(),
                summary: "Extract warning-like lines from workflow run logs.".to_owned(),
                usage: "run warnings <run-id> [--max-body-bytes BYTES] [--max-expanded-bytes BYTES]".to_owned(),
                examples: vec![ManualExample::new("List run warnings", &["run", "warnings", "25451983278"])],
            },
            ManualCommand {
                name: "run artifacts".to_owned(),
                summary: "List workflow run artifacts.".to_owned(),
                usage: "run artifacts <run-id>".to_owned(),
                examples: vec![ManualExample::new("List run artifacts", &["run", "artifacts", "25451983278"])],
            },
        ],
        notes: vec![
            "GitHub-specific features live in this dynamic plugin; local Git commands stay in `ah git`.".to_owned(),
            "Repository defaults to GitHub owner/repo parsed from `origin`; override with --repo OWNER/REPO.".to_owned(),
            "Authentication checks --token, GITHUB_TOKEN, GH_TOKEN, then the Git credential helper; pass --use-git-credential=false to skip the helper.".to_owned(),
            "GITHUB_TOKEN, GH_TOKEN and the credential helper reach only GitHub itself, the detected remote host, or loopback; any other --api-url needs an explicit --token.".to_owned(),
            "Tokens are never sent to a cleartext --api-url unless the host is loopback.".to_owned(),
            "Use global --json for stable machine-readable output and --limit to cap runs/log matches.".to_owned(),
            "Run logs default to an 8 MiB archive budget and 32 MiB expanded budget; override with command-local max byte flags.".to_owned(),
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
                let parse_result = GithubCli::try_parse_from(args.clone());
                assert!(
                    parse_result.is_ok(),
                    "manual example failed to parse for command '{}': argv={args:?}",
                    command.name
                );
            }
        }
    }
}
