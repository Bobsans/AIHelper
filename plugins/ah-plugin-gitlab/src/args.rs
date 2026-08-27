//! The command-line surface: one `Args` struct per command, plus the
//! defaults `serde` and `schemars` need to see as functions.
//!
//! Declaration only. The connection block is `global = true` so every
//! subcommand accepts it, and every leaf struct is `deny_unknown_fields` so a
//! typed call with a stray property is refused rather than ignored.

use super::*;

#[derive(Debug, Parser)]
#[command(name = "gitlab", about = "GitLab release and pipeline helpers")]
pub(crate) struct GitlabCli {
    #[command(flatten)]
    pub(crate) connection: GitlabConnectionArgs,
    #[command(subcommand)]
    pub(crate) command: GitlabCommand,
}

#[derive(Debug, Args, Clone, Deserialize, JsonSchema)]
pub(crate) struct GitlabConnectionArgs {
    #[arg(long, global = true, value_name = "PATH_OR_ID")]
    #[schemars(
        description = "Project override as path (group/subgroup/name) or numeric id. Omit it to read the project from the git remote, which also needs context.cwd."
    )]
    pub(crate) project: Option<String>,
    #[arg(long, global = true, default_value = DEFAULT_REMOTE, value_name = "NAME")]
    #[serde(default = "default_remote")]
    #[schemars(default = "default_remote", length(min = 1))]
    pub(crate) remote: String,
    /// Left unset the default is assumed, which also allows falling back to the
    /// host named by the git remote.
    #[arg(long, global = true, value_name = "URL")]
    #[schemars(
        default = "default_host",
        length(min = 1),
        description = "GitLab base URL. Omit it and the host is taken from the git remote, which is why a self-managed instance needs no override. Set it together with project, since naming the project stops the remote from being read."
    )]
    pub(crate) host: Option<String>,
    #[arg(long, global = true, value_name = "URL")]
    #[schemars(description = "REST API base URL. A supplied token is sent to this host.")]
    pub(crate) api_url: Option<String>,
    #[arg(long, global = true, value_name = "URL")]
    #[schemars(description = "GraphQL API URL used for issue designs.")]
    pub(crate) graphql_url: Option<String>,
    #[arg(long, global = true, value_name = "TOKEN")]
    #[schemars(description = "Explicit GitLab token; prefer environment-based authentication.")]
    pub(crate) token: Option<String>,
    #[arg(
        long,
        global = true,
        default_value_t = true,
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    #[serde(default = "enabled")]
    #[schemars(
        default = "enabled",
        description = "Use Git credential helper lookup as the final fallback."
    )]
    pub(crate) use_git_credential: bool,
    #[arg(long, global = true, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(default = "default_timeout_secs")]
    #[schemars(
        default = "default_timeout_secs",
        range(min = 1),
        description = "Per-request HTTP timeout."
    )]
    pub(crate) timeout_secs: u64,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    pub(crate) cwd: Option<PathBuf>,
}

pub(crate) fn default_remote() -> String {
    DEFAULT_REMOTE.to_owned()
}

pub(crate) fn default_host() -> String {
    DEFAULT_HOST.to_owned()
}

pub(crate) fn enabled() -> bool {
    true
}

pub(crate) fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Subcommand)]
pub(crate) enum GitlabCommand {
    #[command(about = "Inspect detected GitLab project")]
    Project,
    #[command(about = "List GitLab issues")]
    Issues(IssuesArgs),
    #[command(about = "Work with GitLab issues")]
    Issue(IssueArgs),
    #[command(about = "List GitLab releases")]
    Releases,
    #[command(about = "Work with GitLab releases")]
    Release(ReleaseArgs),
    #[command(about = "List GitLab pipelines")]
    Pipelines(PipelinesArgs),
    #[command(about = "Inspect GitLab pipeline")]
    Pipeline(PipelineArgs),
    #[command(about = "Inspect GitLab job")]
    Job(JobArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssuesArgs {
    #[arg(long, default_value = "opened", value_parser = ["opened", "closed", "all"])]
    #[serde(default = "default_issue_state")]
    #[schemars(default = "default_issue_state", extend("enum" = ["opened", "closed", "all"]))]
    pub(crate) state: String,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    /// Optional issue filter.
    #[arg(long)]
    pub(crate) assignee: Option<String>,
    /// Optional issue filter.
    #[arg(long)]
    pub(crate) author: Option<String>,
    /// Optional issue filter.
    #[arg(long)]
    pub(crate) since: Option<String>,
    /// Optional issue filter.
    #[arg(long)]
    pub(crate) search: Option<String>,
}

pub(crate) fn default_issue_state() -> String {
    "opened".to_owned()
}

#[derive(Debug, Args)]
pub(crate) struct IssueArgs {
    #[command(subcommand)]
    pub(crate) command: IssueCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IssueCommand {
    #[command(about = "View issue metadata")]
    View(IssueViewArgs),
    #[command(about = "Create an issue")]
    Create(CreateIssueArgs),
    #[command(about = "Update an issue")]
    Update(UpdateIssueArgs),
    #[command(about = "Close an issue")]
    Close(CloseIssueArgs),
    #[command(about = "Add an issue comment")]
    Comment(CommentIssueArgs),
    #[command(about = "List issue comments")]
    Comments(IssueIidArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueIidArgs {
    /// Project issue iid.
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueViewArgs {
    /// Project issue iid.
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    /// Also load comments and designs.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) full: bool,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateIssueArgs {
    /// Issue title.
    #[arg(long)]
    #[schemars(length(min = 1))]
    pub(crate) title: String,
    /// Inline text.
    #[arg(long, value_name = "TEXT")]
    pub(crate) description: Option<String>,
    /// UTF-8 text file resolved against the execution cwd.
    #[arg(long, value_name = "PATH")]
    pub(crate) description_file: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    #[arg(long = "assignee-id", value_name = "ID")]
    #[serde(default)]
    #[schemars(inner(range(min = 1)))]
    pub(crate) assignee_ids: Vec<u64>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateIssueArgs {
    /// Project issue iid.
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    /// Replacement title.
    #[arg(long)]
    pub(crate) title: Option<String>,
    /// Inline text.
    #[arg(long, value_name = "TEXT")]
    pub(crate) description: Option<String>,
    /// UTF-8 text file resolved against the execution cwd.
    #[arg(long, value_name = "PATH")]
    pub(crate) description_file: Option<String>,
    #[arg(long, value_parser = ["opened", "closed"])]
    #[schemars(extend("enum" = ["opened", "closed"]))]
    pub(crate) state: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub(crate) labels: Vec<String>,
    #[arg(long = "assignee-id", value_name = "ID")]
    #[serde(default)]
    #[schemars(length(min = 1), inner(range(min = 1)))]
    pub(crate) assignee_ids: Vec<u64>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloseIssueArgs {
    /// Project issue iid.
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    /// Inline text.
    #[arg(long, value_name = "TEXT")]
    pub(crate) comment: Option<String>,
    /// UTF-8 text file resolved against the execution cwd.
    #[arg(long, value_name = "PATH")]
    pub(crate) comment_file: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommentIssueArgs {
    /// Project issue iid.
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    /// Inline text.
    #[arg(long, value_name = "TEXT")]
    pub(crate) body: Option<String>,
    /// UTF-8 text file resolved against the execution cwd.
    #[arg(long, value_name = "PATH")]
    pub(crate) body_file: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ReleaseArgs {
    #[command(subcommand)]
    pub(crate) command: ReleaseCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReleaseCommand {
    #[command(about = "Get release metadata by tag")]
    Get(TagArgs),
    #[command(about = "Create a GitLab release for an existing or new tag")]
    Create(CreateReleaseArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagArgs {
    /// Release tag.
    #[schemars(length(min = 1))]
    pub(crate) tag: String,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateReleaseArgs {
    /// Release tag.
    #[schemars(length(min = 1))]
    pub(crate) tag: String,
    /// Release name.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Inline text.
    #[arg(long, value_name = "TEXT")]
    pub(crate) description: Option<String>,
    /// UTF-8 text file resolved against the execution cwd.
    #[arg(long, value_name = "PATH")]
    pub(crate) description_file: Option<String>,
    /// Tag target reference.
    #[arg(long)]
    pub(crate) r#ref: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PipelinesArgs {
    /// Branch filter.
    #[arg(long, value_name = "BRANCH")]
    pub(crate) branch: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct PipelineArgs {
    #[command(subcommand)]
    pub(crate) command: PipelineCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PipelineCommand {
    #[command(about = "Get pipeline metadata")]
    Get(PipelineIdArgs),
    #[command(about = "Wait for pipeline completion")]
    Wait(WaitPipelineArgs),
    #[command(about = "List pipeline jobs")]
    Jobs(PipelineIdArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PipelineIdArgs {
    #[schemars(range(min = 1))]
    pub(crate) pipeline_id: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitPipelineArgs {
    #[schemars(range(min = 1))]
    pub(crate) pipeline_id: u64,
    /// Polling interval.
    #[arg(long, default_value_t = DEFAULT_WAIT_INTERVAL_SECS, value_name = "SECONDS")]
    #[serde(default = "default_wait_interval_secs")]
    #[schemars(default = "default_wait_interval_secs", range(min = 1))]
    pub(crate) interval_secs: u64,
    /// Maximum wait duration.
    #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(rename = "wait_timeout_secs", default = "default_wait_timeout_secs")]
    #[schemars(default = "default_wait_timeout_secs", range(min = 1))]
    pub(crate) timeout_secs: u64,
    /// Return an error for a non-success status.
    #[arg(long)]
    #[serde(default)]
    #[schemars(default)]
    pub(crate) fail_on_failure: bool,
}

pub(crate) fn default_wait_interval_secs() -> u64 {
    DEFAULT_WAIT_INTERVAL_SECS
}

pub(crate) fn default_wait_timeout_secs() -> u64 {
    DEFAULT_WAIT_TIMEOUT_SECS
}

#[derive(Debug, Args)]
pub(crate) struct JobArgs {
    #[command(subcommand)]
    pub(crate) command: JobCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum JobCommand {
    #[command(about = "Read job trace")]
    Trace(JobTraceArgs),
    #[command(about = "Extract warning-like lines from job trace")]
    Warnings(JobTraceReadArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub(crate) struct JobTraceArgs {
    #[schemars(range(min = 1))]
    pub(crate) job_id: u64,
    /// Optional text filter.
    #[arg(long)]
    pub(crate) grep: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub(crate) limits: JobTraceLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub(crate) struct JobTraceReadArgs {
    #[schemars(range(min = 1))]
    pub(crate) job_id: u64,
    #[command(flatten)]
    #[serde(flatten)]
    pub(crate) limits: JobTraceLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
pub(crate) struct JobTraceLimitArgs {
    /// Maximum trace response bytes.
    #[arg(
        long,
        default_value_t = DEFAULT_MAX_TRACE_BODY_BYTES,
        value_name = "BYTES"
    )]
    #[serde(default = "default_max_trace_body_bytes")]
    #[schemars(default = "default_max_trace_body_bytes", range(min = 1))]
    pub(crate) max_body_bytes: usize,
}

pub(crate) fn default_max_trace_body_bytes() -> usize {
    DEFAULT_MAX_TRACE_BODY_BYTES
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn parser_builds_command_tree() {
        let _ = GitlabCli::command();
    }

    #[test]
    fn cli_uses_git_credentials_by_default() {
        let cli = GitlabCli::try_parse_from(["gitlab", "project"]).unwrap();

        assert!(cli.connection.use_git_credential);
    }

    #[test]
    fn cli_allows_disabling_git_credentials() {
        let cli = GitlabCli::try_parse_from(["gitlab", "--use-git-credential=false", "project"])
            .expect("the default credential lookup must be opt-out capable");

        assert!(!cli.connection.use_git_credential);
    }
}
