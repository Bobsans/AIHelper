//! The command-line surface: one `Args` struct per command, plus the
//! defaults `serde` and `schemars` need to see as functions.
//!
//! Declaration only. The connection block is `global = true` so every
//! subcommand accepts it, and every leaf struct is `deny_unknown_fields` so a
//! typed call with a stray property is refused rather than ignored.

use super::*;

#[derive(Debug, Parser)]
#[command(name = "github", about = "GitHub release and workflow helpers")]
pub(crate) struct GithubCli {
    #[command(flatten)]
    pub(crate) connection: GithubConnectionArgs,
    #[command(subcommand)]
    pub(crate) command: GithubCommand,
}

#[derive(Debug, Args, Clone, Deserialize, JsonSchema)]
pub(crate) struct GithubConnectionArgs {
    #[arg(long, global = true, value_name = "OWNER/REPO")]
    #[schemars(
        description = "Repository override in OWNER/REPO form. Omit it to read the repository from the git remote, which also needs context.cwd."
    )]
    pub(crate) repo: Option<String>,
    #[arg(long, global = true, default_value = DEFAULT_REMOTE, value_name = "NAME")]
    #[serde(default = "default_remote")]
    #[schemars(default = "default_remote", length(min = 1))]
    pub(crate) remote: String,
    #[arg(long, global = true, default_value = DEFAULT_API_URL, value_name = "URL")]
    #[serde(default = "default_api_url")]
    #[schemars(
        default = "default_api_url",
        length(min = 1),
        description = "GitHub-compatible API base URL. A supplied token is sent to this host."
    )]
    pub(crate) api_url: String,
    #[arg(long, global = true, value_name = "TOKEN")]
    #[schemars(description = "Explicit GitHub token; prefer environment-based authentication.")]
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

pub(crate) fn default_api_url() -> String {
    DEFAULT_API_URL.to_owned()
}

pub(crate) fn enabled() -> bool {
    true
}

pub(crate) fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Subcommand)]
pub(crate) enum GithubCommand {
    #[command(about = "Inspect detected GitHub repository")]
    Repo,
    #[command(about = "List GitHub issues")]
    Issues(IssuesArgs),
    #[command(about = "Work with GitHub issues")]
    Issue(IssueArgs),
    #[command(about = "Work with GitHub releases")]
    Release(ReleaseArgs),
    #[command(about = "List GitHub Actions workflows")]
    Workflows,
    #[command(about = "Dispatch a GitHub Actions workflow")]
    Workflow(WorkflowArgs),
    #[command(about = "List GitHub Actions workflow runs")]
    Runs(RunsArgs),
    #[command(about = "Inspect a GitHub Actions workflow run")]
    Run(RunArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssuesArgs {
    #[arg(long, default_value = "open", value_parser = ["open", "closed", "all"])]
    #[serde(default = "default_issue_state")]
    #[schemars(default = "default_issue_state", extend("enum" = ["open", "closed", "all"]))]
    pub(crate) state: String,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(description = "Issue labels.")]
    pub(crate) labels: Vec<String>,
    #[arg(long)]
    #[schemars(description = "Assignee login.")]
    pub(crate) assignee: Option<String>,
    #[arg(long)]
    #[schemars(description = "Author login.")]
    pub(crate) author: Option<String>,
    #[arg(long)]
    #[schemars(description = "ISO date or timestamp.")]
    pub(crate) since: Option<String>,
    #[arg(long)]
    #[schemars(description = "GitHub search query.")]
    pub(crate) search: Option<String>,
}

pub(crate) fn default_issue_state() -> String {
    "open".to_owned()
}

#[derive(Debug, Args)]
pub(crate) struct IssueArgs {
    #[command(subcommand)]
    pub(crate) command: IssueCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum IssueCommand {
    #[command(about = "View issue metadata")]
    View(IssueNumberArgs),
    #[command(about = "Create an issue")]
    Create(CreateIssueArgs),
    #[command(about = "Update an issue")]
    Update(UpdateIssueArgs),
    #[command(about = "Close an issue")]
    Close(CloseIssueArgs),
    #[command(about = "Add an issue comment")]
    Comment(CommentIssueArgs),
    #[command(about = "List issue comments")]
    Comments(IssueNumberArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueNumberArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    pub(crate) number: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateIssueArgs {
    #[arg(long)]
    #[schemars(length(min = 1), description = "Issue title.")]
    pub(crate) title: String,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    pub(crate) body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    pub(crate) body_file: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(description = "Labels to set.")]
    pub(crate) labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    #[serde(default)]
    #[schemars(description = "Assignees to set.")]
    pub(crate) assignees: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    pub(crate) number: u64,
    #[arg(long)]
    #[schemars(description = "Replacement title.")]
    pub(crate) title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    pub(crate) body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    pub(crate) body_file: Option<String>,
    #[arg(long, value_parser = ["open", "closed"])]
    #[schemars(extend("enum" = ["open", "closed"]))]
    pub(crate) state: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub(crate) labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    #[serde(default)]
    #[schemars(length(min = 1))]
    pub(crate) assignees: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CloseIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    pub(crate) number: u64,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    pub(crate) comment: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    pub(crate) comment_file: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommentIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    pub(crate) number: u64,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    pub(crate) body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
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
    #[command(about = "List release assets by tag")]
    Assets(TagArgs),
    #[command(about = "Create a GitHub release for an existing or new tag")]
    Create(CreateReleaseArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagArgs {
    #[schemars(length(min = 1), description = "Release tag.")]
    pub(crate) tag: String,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateReleaseArgs {
    #[schemars(length(min = 1), description = "Release tag.")]
    pub(crate) tag: String,
    #[arg(long)]
    #[schemars(description = "Release title.")]
    pub(crate) title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    pub(crate) notes: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    pub(crate) notes_file: Option<String>,
    #[arg(long)]
    #[schemars(description = "Target commit-ish.")]
    pub(crate) target: Option<String>,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Create as draft.")]
    pub(crate) draft: bool,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Mark as prerelease.")]
    pub(crate) prerelease: bool,
}

#[derive(Debug, Args)]
pub(crate) struct WorkflowArgs {
    #[command(subcommand)]
    pub(crate) command: WorkflowCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowCommand {
    #[command(about = "Dispatch a workflow by id or file name")]
    Run(WorkflowRunArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowRunArgs {
    #[schemars(length(min = 1), description = "Workflow id or file name.")]
    pub(crate) workflow: String,
    #[arg(long, value_name = "REF")]
    #[schemars(length(min = 1), description = "Git reference to dispatch.")]
    pub(crate) r#ref: String,
    #[arg(long = "input", value_name = "KEY=VALUE")]
    #[serde(default)]
    #[schemars(
        inner(pattern(r"^[^=]+=.*$")),
        description = "Workflow inputs encoded as KEY=VALUE."
    )]
    pub(crate) inputs: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunsArgs {
    #[arg(long, value_name = "WORKFLOW")]
    #[schemars(description = "Workflow id or file.")]
    pub(crate) workflow: Option<String>,
    #[arg(long, value_name = "BRANCH")]
    #[schemars(description = "Head branch filter.")]
    pub(crate) branch: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    #[command(subcommand)]
    pub(crate) command: RunCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunCommand {
    #[command(about = "Get workflow run metadata")]
    Get(RunIdArgs),
    #[command(about = "Wait for workflow run completion")]
    Wait(WaitRunArgs),
    #[command(about = "List workflow run jobs")]
    Jobs(RunIdArgs),
    #[command(about = "Search workflow run logs")]
    Logs(LogArgs),
    #[command(about = "Extract warning-like lines from workflow run logs")]
    Warnings(LogReadArgs),
    #[command(about = "List workflow run artifacts")]
    Artifacts(RunIdArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunIdArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    pub(crate) run_id: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitRunArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    pub(crate) run_id: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_INTERVAL_SECS, value_name = "SECONDS")]
    #[serde(default = "default_wait_interval_secs")]
    #[schemars(
        default = "default_wait_interval_secs",
        range(min = 1),
        description = "Polling interval."
    )]
    pub(crate) interval_secs: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(rename = "wait_timeout_secs", default = "default_wait_timeout_secs")]
    #[schemars(
        default = "default_wait_timeout_secs",
        range(min = 1),
        description = "Maximum wait duration."
    )]
    pub(crate) timeout_secs: u64,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Return an error for a non-success conclusion.")]
    pub(crate) fail_on_failure: bool,
}

pub(crate) fn default_wait_interval_secs() -> u64 {
    DEFAULT_WAIT_INTERVAL_SECS
}

pub(crate) fn default_wait_timeout_secs() -> u64 {
    DEFAULT_WAIT_TIMEOUT_SECS
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub(crate) struct LogArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    pub(crate) run_id: u64,
    #[arg(long)]
    #[schemars(description = "Optional text filter.")]
    pub(crate) grep: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    pub(crate) limits: LogLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
pub(crate) struct LogReadArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    pub(crate) run_id: u64,
    #[command(flatten)]
    #[serde(flatten)]
    pub(crate) limits: LogLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
pub(crate) struct LogLimitArgs {
    #[arg(long, default_value_t = DEFAULT_MAX_LOG_BODY_BYTES, value_name = "BYTES")]
    #[serde(default = "default_max_log_body_bytes")]
    #[schemars(
        default = "default_max_log_body_bytes",
        range(min = 1),
        description = "Maximum compressed response bytes."
    )]
    pub(crate) max_body_bytes: usize,
    #[arg(
        long,
        default_value_t = DEFAULT_MAX_EXPANDED_LOG_BYTES,
        value_name = "BYTES"
    )]
    #[serde(default = "default_max_expanded_log_bytes")]
    #[schemars(
        default = "default_max_expanded_log_bytes",
        range(min = 1),
        description = "Maximum expanded archive bytes."
    )]
    pub(crate) max_expanded_bytes: usize,
}

pub(crate) fn default_max_log_body_bytes() -> usize {
    DEFAULT_MAX_LOG_BODY_BYTES
}

pub(crate) fn default_max_expanded_log_bytes() -> usize {
    DEFAULT_MAX_EXPANDED_LOG_BYTES
}

pub(crate) fn parse_key_values(
    values: &[String],
    flag_name: &str,
) -> Result<Value, InvocationResponse> {
    let mut map = serde_json::Map::new();
    for value in values {
        let Some((key, raw_value)) = value.split_once('=') else {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("{flag_name} must use KEY=VALUE format, got '{value}'"),
            ));
        };
        if key.trim().is_empty() {
            return Err(InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("{flag_name} key must not be empty"),
            ));
        }
        map.insert(key.to_owned(), Value::String(raw_value.to_owned()));
    }
    Ok(Value::Object(map))
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn parser_builds_command_tree() {
        let _ = GithubCli::command();
    }

    #[test]
    fn cli_uses_git_credentials_by_default() {
        let cli = GithubCli::try_parse_from(["github", "repo"]).unwrap();

        assert!(cli.connection.use_git_credential);
    }

    #[test]
    fn cli_allows_disabling_git_credentials() {
        let cli = GithubCli::try_parse_from(["github", "--use-git-credential=false", "repo"])
            .expect("the default credential lookup must be opt-out capable");

        assert!(!cli.connection.use_git_credential);
    }

    #[test]
    fn parses_workflow_inputs() {
        let parsed = parse_key_values(
            &["target=main".to_owned(), "dry_run=true".to_owned()],
            "--input",
        )
        .expect("inputs should parse");
        assert_eq!(parsed["target"], "main");
        assert_eq!(parsed["dry_run"], "true");
    }
}
