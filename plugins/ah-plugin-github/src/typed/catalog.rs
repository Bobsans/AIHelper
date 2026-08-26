//! What `github` publishes to the command catalog: one descriptor per
//! command, with its schemas, declared effects, risk and secret slot.
//!
//! Declaration, not logic - which is why it outgrew the dispatch it used to
//! share a file with.

use super::*;

pub(crate) fn command_catalog() -> CommandCatalog {
    CommandCatalog::new(
        PLUGIN_NAME,
        DOMAIN,
        vec![
            repo_descriptor(),
            issues_descriptor(),
            issue_view_descriptor(),
            issue_create_descriptor(),
            issue_update_descriptor(),
            issue_close_descriptor(),
            issue_comment_descriptor(),
            issue_comments_descriptor(),
            release_get_descriptor(),
            release_assets_descriptor(),
            release_create_descriptor(),
            workflows_descriptor(),
            workflow_run_descriptor(),
            runs_descriptor(),
            run_get_descriptor(),
            run_wait_descriptor(),
            run_jobs_descriptor(),
            run_logs_descriptor(false),
            run_logs_descriptor(true),
            run_artifacts_descriptor(),
        ],
    )
}

pub(super) fn repo_descriptor() -> CommandDescriptor {
    descriptor(
        "github.repo",
        "Inspect GitHub repository",
        "Detect the GitHub repository and return remote plus API metadata.",
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<RepoOutput>("github.repo"),
        read_effects(
            "May run Git repository detection and sends a read request to the configured API URL; a supplied token is sent to that host.",
        ),
    )
}

pub(super) fn issues_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issues",
        "List GitHub issues",
        "List or search repository issues with filters and the shared result limit.",
        input_schema_for::<Wire<IssuesArgs>>(),
        output_schema_for::<IssuesOutput>("github.issues"),
        read_effects(
            "Reads issue metadata from the configured GitHub API and may expose private repository data.",
        ),
    )
}

pub(super) fn issue_view_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.view",
        "View GitHub issue",
        "Return one GitHub issue by repository issue number.",
        input_schema_for::<Wire<IssueNumberArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.view"),
        read_effects("Reads one issue and its metadata from the configured GitHub API."),
    )
}

pub(super) fn issue_create_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.create",
        "Create GitHub issue",
        "Create a repository issue with optional body, labels, and assignees. Use body or body_file, not both.",
        input_schema_for::<Wire<CreateIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.create"),
        write_effects(
            "Creates a persistent issue and may notify repository participants; body files are read from the execution cwd.",
        ),
    )
}

pub(super) fn issue_update_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.update",
        "Update GitHub issue",
        "Update one or more issue fields. At least one update field is required; use body or body_file, not both.",
        input_schema_for::<Wire<UpdateIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.update"),
        write_effects(
            "Mutates a persistent issue and may change workflow state or notify participants.",
        ),
    )
}

pub(super) fn issue_close_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.close",
        "Close GitHub issue",
        "Close an issue, optionally adding a comment first. Use comment or comment_file, not both.",
        input_schema_for::<Wire<CloseIssueArgs>>(),
        output_schema_for::<IssueOutput>("github.issue.close"),
        write_effects(
            "May create a comment, closes a persistent issue, and may notify participants.",
        ),
    )
}

pub(super) fn issue_comment_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.comment",
        "Comment on GitHub issue",
        "Create a comment on one repository issue. Exactly one of body or body_file is required.",
        input_schema_for::<Wire<CommentIssueArgs>>(),
        output_schema_for::<IssueCommentOutput>("github.issue.comment"),
        write_effects("Creates a persistent issue comment and may notify repository participants."),
    )
}

pub(super) fn issue_comments_descriptor() -> CommandDescriptor {
    descriptor(
        "github.issue.comments",
        "List GitHub issue comments",
        "List comments for one issue with the shared result limit.",
        input_schema_for::<Wire<IssueNumberArgs>>(),
        output_schema_for::<IssueCommentsOutput>("github.issue.comments"),
        read_effects("Reads issue comments and author metadata from the configured GitHub API."),
    )
}

pub(super) fn release_get_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.get",
        "Get GitHub release",
        "Return GitHub release metadata by tag.",
        input_schema_for::<Wire<TagArgs>>(),
        output_schema_for::<ReleaseOutput>("github.release.get"),
        read_effects("Reads release metadata and asset URLs from the configured GitHub API."),
    )
}

pub(super) fn release_assets_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.assets",
        "List GitHub release assets",
        "List assets attached to a release tag.",
        input_schema_for::<Wire<TagArgs>>(),
        output_schema_for::<ReleaseAssetsOutput>("github.release.assets"),
        read_effects(
            "Reads release asset metadata and download URLs from the configured GitHub API.",
        ),
    )
}

pub(super) fn release_create_descriptor() -> CommandDescriptor {
    descriptor(
        "github.release.create",
        "Create GitHub release",
        "Create a GitHub release for a tag with optional notes and flags. Use notes or notes_file, not both.",
        input_schema_for::<Wire<CreateReleaseArgs>>(),
        output_schema_for::<ReleaseOutput>("github.release.create"),
        write_effects(
            "Creates a persistent release and may create or resolve a tag target; release notes files are read from the execution cwd.",
        ),
    )
}

pub(super) fn workflows_descriptor() -> CommandDescriptor {
    descriptor(
        "github.workflows",
        "List GitHub workflows",
        "List GitHub Actions workflows in the repository.",
        input_schema_for::<Wire<NoArgs>>(),
        output_schema_for::<WorkflowsOutput>("github.workflows"),
        read_effects(
            "Reads workflow names, paths, states, and URLs from the configured GitHub API.",
        ),
    )
}

pub(super) fn workflow_run_descriptor() -> CommandDescriptor {
    descriptor(
        "github.workflow.run",
        "Dispatch GitHub workflow",
        "Dispatch a GitHub Actions workflow on a reference. Dispatching needs a token carrying the actions write scope, which the git credential helper usually does not: on HTTP 401 stop and ask the user for a github-token secret rather than retrying.",
        input_schema_for::<Wire<WorkflowRunArgs>>(),
        output_schema_for::<WorkflowDispatchOutput>("github.workflow.run"),
        write_effects(
            "Starts an external workflow that may execute arbitrary repository automation and consume billed resources.",
        ),
    )
}

pub(super) fn runs_descriptor() -> CommandDescriptor {
    descriptor(
        "github.runs",
        "List GitHub workflow runs",
        "List workflow runs with optional workflow and branch filters.",
        input_schema_for::<Wire<RunsArgs>>(),
        output_schema_for::<RunsOutput>("github.runs"),
        read_effects(
            "Reads workflow run status, commit SHA, and URLs from the configured GitHub API.",
        ),
    )
}

pub(super) fn run_get_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.get",
        "Get GitHub workflow run",
        "Return one workflow run by numeric id.",
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<RunOutput>("github.run.get"),
        read_effects(
            "Reads one workflow run and its commit/status metadata from the configured GitHub API.",
        ),
    )
}

pub(super) fn run_wait_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.wait",
        "Wait for GitHub workflow run",
        "Poll a workflow run until completion, timeout, or cancellation.",
        input_schema_for::<Wire<WaitRunArgs>>(),
        output_schema_for::<WaitRunOutput>("github.run.wait"),
        read_effects(
            "Repeatedly reads external workflow state until completion and may consume API rate limits.",
        ),
    )
}

pub(super) fn run_jobs_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.jobs",
        "List GitHub workflow jobs",
        "List jobs belonging to one workflow run.",
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<JobsOutput>("github.run.jobs"),
        read_effects(
            "Reads workflow job names, status, timestamps, and URLs from the configured GitHub API.",
        ),
    )
}

pub(super) fn run_logs_descriptor(warnings: bool) -> CommandDescriptor {
    let id = if warnings {
        "github.run.warnings"
    } else {
        "github.run.logs"
    };
    descriptor(
        id,
        if warnings {
            "Extract GitHub run warnings"
        } else {
            "Search GitHub run logs"
        },
        if warnings {
            "Extract warning-like lines from one workflow run log archive. The archive exists only once the run has finished and until GitHub expires it, so an unfinished or expired run answers 404: check github.run.get, or wait with github.run.wait, instead of retrying."
        } else {
            "Read or filter lines from one workflow run log archive. The archive exists only once the run has finished and until GitHub expires it, so an unfinished or expired run answers 404: check github.run.get, or wait with github.run.wait, instead of retrying."
        },
        if warnings {
            input_schema_for::<Wire<LogReadArgs>>()
        } else {
            input_schema_for::<Wire<LogArgs>>()
        },
        output_schema_for::<LogsOutput>(id),
        read_effects(
            "Downloads and expands workflow logs, which may contain secrets or untrusted build output.",
        ),
    )
}

pub(super) fn run_artifacts_descriptor() -> CommandDescriptor {
    descriptor(
        "github.run.artifacts",
        "List GitHub workflow artifacts",
        "List artifacts produced by one workflow run.",
        input_schema_for::<Wire<RunIdArgs>>(),
        output_schema_for::<ArtifactsOutput>("github.run.artifacts"),
        read_effects(
            "Reads artifact names, sizes, expiry state, and archive URLs from the configured GitHub API.",
        ),
    )
}

/// Every command in this domain talks to the API, so all of them accept the
/// vault token slot.
pub(super) fn token_slot() -> SecretSlot {
    SecretSlot::optional("token", ["github-token"], "GitHub API token.")
}

pub(super) fn read_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        true,
        false,
        true,
        true,
        vec![
            CommandEffect::NetworkRead,
            CommandEffect::ExternalRead,
            CommandEffect::ConfigurationRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::Medium,
        impact,
        Reversibility::Yes,
    )
}

pub(super) fn write_effects(impact: &str) -> CommandEffects {
    CommandEffects::new(
        false,
        false,
        false,
        true,
        vec![
            CommandEffect::NetworkWrite,
            CommandEffect::ExternalWrite,
            CommandEffect::ConfigurationRead,
            CommandEffect::FilesystemRead,
            CommandEffect::ProcessSpawn,
        ],
        RiskLevel::High,
        impact,
        Reversibility::Unknown,
    )
}
