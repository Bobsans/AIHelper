use std::{
    fs,
    io::{BufRead, BufReader, Cursor, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[cfg(test)]
use ah_plugin_api::InvocationRequest;
use ah_plugin_sdk::{credentials, http, render};

use ah_plugin_api::{
    GlobalOptionsWire, InvocationResponse, ManualCommand, ManualExample, PluginManual,
    TextFormatter, TextStyle, noninteractive_command,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::{Method, blocking::Client};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
#[cfg(test)]
use std::thread;
use zip::ZipArchive;

const DOMAIN: &str = "github";
const PLUGIN_NAME: &str = "external-github";
const DESCRIPTION: &str = "GitHub Releases and Actions plugin (dynamic)";
const DEFAULT_API_URL: &str = "https://api.github.com";
const DEFAULT_API_AUTHORITY: &str = "api.github.com";
const DEFAULT_REMOTE: &str = "origin";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 15;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_MAX_LOG_BODY_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_MAX_EXPANDED_LOG_BYTES: usize = 32 * 1024 * 1024;
const GIT_CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(5);

static PLUGIN_NAME_C: &[u8] = b"external-github\0";
static DOMAIN_C: &[u8] = b"github\0";
static DESCRIPTION_C: &[u8] = b"GitHub Releases and Actions plugin (dynamic)\0";

#[cfg(test)]
mod snapshots;
mod typed;

ah_plugin_api::define_plugin_entrypoint_v1!(
    plugin_name_c: PLUGIN_NAME_C,
    domain_c: DOMAIN_C,
    description_c: DESCRIPTION_C,
    domain: DOMAIN,
    parse_fn: parse_args,
    execute_fn: execute,
    manual_fn: plugin_manual,
    typed_catalog_fn: typed::command_catalog,
    typed_execute_fn: typed::invoke,
    typed_cancel_fn: ah_plugin_api::cancellation::cancel,
);

impl ah_plugin_api::BindResolvedSecrets for GithubCli {
    fn bind_resolved_secrets(
        &mut self,
        secrets: &std::collections::BTreeMap<String, ah_plugin_api::ResolvedSecret>,
    ) -> Result<(), InvocationResponse> {
        if let Some(token) = typed::token_from_resolved_secrets(secrets)
            .map_err(|error| error.with_error_domain(DOMAIN))?
        {
            if self.connection.token.is_some() {
                return Err(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    "GitHub token credential conflicts with an inline --token",
                )
                .with_error_domain(DOMAIN));
            }
            self.connection.token = Some(token);
        }
        Ok(())
    }
}

#[derive(Debug, Parser)]
#[command(name = "github", about = "GitHub release and workflow helpers")]
struct GithubCli {
    #[command(flatten)]
    connection: GithubConnectionArgs,
    #[command(subcommand)]
    command: GithubCommand,
}

#[derive(Debug, Args, Clone, Deserialize, JsonSchema)]
struct GithubConnectionArgs {
    #[arg(long, global = true, value_name = "OWNER/REPO")]
    #[schemars(
        description = "Repository override in OWNER/REPO form. Omit it to read the repository from the git remote, which also needs context.cwd."
    )]
    repo: Option<String>,
    #[arg(long, global = true, default_value = DEFAULT_REMOTE, value_name = "NAME")]
    #[serde(default = "default_remote")]
    #[schemars(default = "default_remote", length(min = 1))]
    remote: String,
    #[arg(long, global = true, default_value = DEFAULT_API_URL, value_name = "URL")]
    #[serde(default = "default_api_url")]
    #[schemars(
        default = "default_api_url",
        length(min = 1),
        description = "GitHub-compatible API base URL. A supplied token is sent to this host."
    )]
    api_url: String,
    #[arg(long, global = true, value_name = "TOKEN")]
    #[schemars(description = "Explicit GitHub token; prefer environment-based authentication.")]
    token: Option<String>,
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
    use_git_credential: bool,
    #[arg(long, global = true, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(default = "default_timeout_secs")]
    #[schemars(
        default = "default_timeout_secs",
        range(min = 1),
        description = "Per-request HTTP timeout."
    )]
    timeout_secs: u64,
    // Supplied by the execution context, never by the caller.
    #[arg(skip)]
    #[serde(skip)]
    cwd: Option<PathBuf>,
}

fn default_remote() -> String {
    DEFAULT_REMOTE.to_owned()
}

fn default_api_url() -> String {
    DEFAULT_API_URL.to_owned()
}

fn enabled() -> bool {
    true
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Subcommand)]
enum GithubCommand {
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
struct IssuesArgs {
    #[arg(long, default_value = "open", value_parser = ["open", "closed", "all"])]
    #[serde(default = "default_issue_state")]
    #[schemars(default = "default_issue_state", extend("enum" = ["open", "closed", "all"]))]
    state: String,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(description = "Issue labels.")]
    labels: Vec<String>,
    #[arg(long)]
    #[schemars(description = "Assignee login.")]
    assignee: Option<String>,
    #[arg(long)]
    #[schemars(description = "Author login.")]
    author: Option<String>,
    #[arg(long)]
    #[schemars(description = "ISO date or timestamp.")]
    since: Option<String>,
    #[arg(long)]
    #[schemars(description = "GitHub search query.")]
    search: Option<String>,
}

fn default_issue_state() -> String {
    "open".to_owned()
}

#[derive(Debug, Args)]
struct IssueArgs {
    #[command(subcommand)]
    command: IssueCommand,
}

#[derive(Debug, Subcommand)]
enum IssueCommand {
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
struct IssueNumberArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    number: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateIssueArgs {
    #[arg(long)]
    #[schemars(length(min = 1), description = "Issue title.")]
    title: String,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    body_file: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(description = "Labels to set.")]
    labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    #[serde(default)]
    #[schemars(description = "Assignees to set.")]
    assignees: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct UpdateIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    number: u64,
    #[arg(long)]
    #[schemars(description = "Replacement title.")]
    title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    body_file: Option<String>,
    #[arg(long, value_parser = ["open", "closed"])]
    #[schemars(extend("enum" = ["open", "closed"]))]
    state: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    #[serde(default)]
    #[schemars(length(min = 1))]
    labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    #[serde(default)]
    #[schemars(length(min = 1))]
    assignees: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    number: u64,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    comment: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    comment_file: Option<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CommentIssueArgs {
    #[schemars(range(min = 1), description = "Issue number.")]
    number: u64,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    body_file: Option<String>,
}

#[derive(Debug, Args)]
struct ReleaseArgs {
    #[command(subcommand)]
    command: ReleaseCommand,
}

#[derive(Debug, Subcommand)]
enum ReleaseCommand {
    #[command(about = "Get release metadata by tag")]
    Get(TagArgs),
    #[command(about = "List release assets by tag")]
    Assets(TagArgs),
    #[command(about = "Create a GitHub release for an existing or new tag")]
    Create(CreateReleaseArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TagArgs {
    #[schemars(length(min = 1), description = "Release tag.")]
    tag: String,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateReleaseArgs {
    #[schemars(length(min = 1), description = "Release tag.")]
    tag: String,
    #[arg(long)]
    #[schemars(description = "Release title.")]
    title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    #[schemars(description = "Inline text.")]
    notes: Option<String>,
    #[arg(long, value_name = "PATH")]
    #[schemars(description = "UTF-8 text file resolved against the execution cwd.")]
    notes_file: Option<String>,
    #[arg(long)]
    #[schemars(description = "Target commit-ish.")]
    target: Option<String>,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Create as draft.")]
    draft: bool,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Mark as prerelease.")]
    prerelease: bool,
}

#[derive(Debug, Args)]
struct WorkflowArgs {
    #[command(subcommand)]
    command: WorkflowCommand,
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    #[command(about = "Dispatch a workflow by id or file name")]
    Run(WorkflowRunArgs),
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowRunArgs {
    #[schemars(length(min = 1), description = "Workflow id or file name.")]
    workflow: String,
    #[arg(long, value_name = "REF")]
    #[schemars(length(min = 1), description = "Git reference to dispatch.")]
    r#ref: String,
    #[arg(long = "input", value_name = "KEY=VALUE")]
    #[serde(default)]
    #[schemars(
        inner(pattern(r"^[^=]+=.*$")),
        description = "Workflow inputs encoded as KEY=VALUE."
    )]
    inputs: Vec<String>,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunsArgs {
    #[arg(long, value_name = "WORKFLOW")]
    #[schemars(description = "Workflow id or file.")]
    workflow: Option<String>,
    #[arg(long, value_name = "BRANCH")]
    #[schemars(description = "Head branch filter.")]
    branch: Option<String>,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[command(subcommand)]
    command: RunCommand,
}

#[derive(Debug, Subcommand)]
enum RunCommand {
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
struct RunIdArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    run_id: u64,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitRunArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    run_id: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_INTERVAL_SECS, value_name = "SECONDS")]
    #[serde(default = "default_wait_interval_secs")]
    #[schemars(
        default = "default_wait_interval_secs",
        range(min = 1),
        description = "Polling interval."
    )]
    interval_secs: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT_SECS, value_name = "SECONDS")]
    #[serde(rename = "wait_timeout_secs", default = "default_wait_timeout_secs")]
    #[schemars(
        default = "default_wait_timeout_secs",
        range(min = 1),
        description = "Maximum wait duration."
    )]
    timeout_secs: u64,
    #[arg(long)]
    #[serde(default)]
    #[schemars(default, description = "Return an error for a non-success conclusion.")]
    fail_on_failure: bool,
}

fn default_wait_interval_secs() -> u64 {
    DEFAULT_WAIT_INTERVAL_SECS
}

fn default_wait_timeout_secs() -> u64 {
    DEFAULT_WAIT_TIMEOUT_SECS
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
struct LogArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    run_id: u64,
    #[arg(long)]
    #[schemars(description = "Optional text filter.")]
    grep: Option<String>,
    #[command(flatten)]
    #[serde(flatten)]
    limits: LogLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
#[schemars(extend("additionalProperties" = false))]
struct LogReadArgs {
    #[schemars(range(min = 1), description = "Workflow run id.")]
    run_id: u64,
    #[command(flatten)]
    #[serde(flatten)]
    limits: LogLimitArgs,
}

#[derive(Debug, Args, Deserialize, JsonSchema)]
struct LogLimitArgs {
    #[arg(long, default_value_t = DEFAULT_MAX_LOG_BODY_BYTES, value_name = "BYTES")]
    #[serde(default = "default_max_log_body_bytes")]
    #[schemars(
        default = "default_max_log_body_bytes",
        range(min = 1),
        description = "Maximum compressed response bytes."
    )]
    max_body_bytes: usize,
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
    max_expanded_bytes: usize,
}

fn default_max_log_body_bytes() -> usize {
    DEFAULT_MAX_LOG_BODY_BYTES
}

fn default_max_expanded_log_bytes() -> usize {
    DEFAULT_MAX_EXPANDED_LOG_BYTES
}

#[derive(Debug, Clone)]
struct RepoSlug {
    owner: String,
    repo: String,
}

impl RepoSlug {
    fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

#[derive(Debug)]
struct GithubContext {
    client: Client,
    api_url: String,
    token: Option<String>,
    repo: RepoSlug,
    remote_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RepoOutput {
    command: &'static str,
    repository: String,
    owner: String,
    name: String,
    remote_url: Option<String>,
    api_url: String,
    html_url: Option<String>,
    default_branch: Option<String>,
    private: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct GithubRepoResponse {
    full_name: Option<String>,
    html_url: Option<String>,
    default_branch: Option<String>,
    private: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct GithubUser {
    login: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct GithubLabel {
    name: String,
}

/// GitHub owns the shape of a linked pull request, and it is absent for a plain
/// issue, so the published schema stays open and nullable.
fn nullable_external_object(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let object = ah_plugin_api::schema::external_object(generator);
    schemars::json_schema!({"oneOf": [object, {"type": "null"}]})
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct IssueResponse {
    #[schemars(range(min = 1))]
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    html_url: Option<String>,
    user: Option<GithubUser>,
    #[serde(default)]
    labels: Vec<GithubLabel>,
    #[serde(default)]
    assignees: Vec<GithubUser>,
    comments: Option<u64>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
    // GitHub owns this sub-object and only its presence matters here.
    #[serde(default)]
    #[schemars(schema_with = "nullable_external_object")]
    pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IssueSearchResponse {
    items: Vec<IssueResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssuesOutput {
    command: &'static str,
    repository: String,
    state: String,
    labels: Vec<String>,
    assignee: Option<String>,
    author: Option<String>,
    since: Option<String>,
    search: Option<String>,
    issue_count: usize,
    issues: Vec<IssueResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueOutput {
    command: &'static str,
    repository: String,
    issue: IssueResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct IssueCommentResponse {
    #[schemars(range(min = 1))]
    id: u64,
    body: Option<String>,
    html_url: Option<String>,
    user: Option<GithubUser>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueCommentsOutput {
    command: &'static str,
    repository: String,
    #[schemars(range(min = 1))]
    number: u64,
    comment_count: usize,
    comments: Vec<IssueCommentResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct IssueCommentOutput {
    command: &'static str,
    repository: String,
    #[schemars(range(min = 1))]
    number: u64,
    comment: IssueCommentResponse,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct ReleaseResponse {
    #[schemars(range(min = 1))]
    id: u64,
    tag_name: String,
    name: Option<String>,
    draft: bool,
    prerelease: bool,
    html_url: Option<String>,
    published_at: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct ReleaseAsset {
    #[schemars(range(min = 1))]
    id: u64,
    name: String,
    size: u64,
    browser_download_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReleaseOutput {
    command: &'static str,
    repository: String,
    release: ReleaseResponse,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReleaseAssetsOutput {
    command: &'static str,
    repository: String,
    tag: String,
    asset_count: usize,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkflowListResponse {
    workflows: Vec<WorkflowResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct WorkflowResponse {
    #[schemars(range(min = 1))]
    id: u64,
    name: String,
    path: String,
    state: String,
    html_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowsOutput {
    command: &'static str,
    repository: String,
    workflow_count: usize,
    workflows: Vec<WorkflowResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkflowDispatchOutput {
    command: &'static str,
    repository: String,
    workflow: String,
    r#ref: String,
    input_count: usize,
    dispatched: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct RunsListResponse {
    workflow_runs: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct WorkflowRunResponse {
    #[schemars(range(min = 1))]
    id: u64,
    name: Option<String>,
    event: String,
    status: String,
    conclusion: Option<String>,
    head_branch: Option<String>,
    head_sha: String,
    html_url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunsOutput {
    command: &'static str,
    repository: String,
    workflow: Option<String>,
    branch: Option<String>,
    run_count: usize,
    runs: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunOutput {
    command: &'static str,
    repository: String,
    run: WorkflowRunResponse,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitRunOutput {
    command: &'static str,
    repository: String,
    run: WorkflowRunResponse,
    elapsed_secs: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct JobsListResponse {
    jobs: Vec<JobResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct JobResponse {
    #[schemars(range(min = 1))]
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JobsOutput {
    command: &'static str,
    repository: String,
    #[schemars(range(min = 1))]
    run_id: u64,
    job_count: usize,
    jobs: Vec<JobResponse>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogLine {
    file: String,
    #[schemars(range(min = 1))]
    line: usize,
    text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogsOutput {
    command: &'static str,
    repository: String,
    #[schemars(range(min = 1))]
    run_id: u64,
    grep: Option<String>,
    match_count: usize,
    truncated: bool,
    matches: Vec<LogLine>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ArtifactsOutput {
    command: &'static str,
    repository: String,
    #[schemars(range(min = 1))]
    run_id: u64,
    artifact_count: usize,
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactsListResponse {
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct ArtifactResponse {
    #[schemars(range(min = 1))]
    id: u64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    archive_download_url: Option<String>,
}

fn parse_args(argv: &[String]) -> Result<GithubCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match GithubCli::try_parse_from(args) {
        Ok(value) => Ok(value),
        Err(error) => {
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                Err(InvocationResponse::ok(Some(error.to_string())))
            } else {
                Err(InvocationResponse::error(
                    "INVALID_ARGUMENT",
                    error.to_string(),
                ))
            }
        }
    }
}

fn execute(cli: GithubCli, globals: &GlobalOptionsWire) -> InvocationResponse {
    let context = match github_context(&cli.connection) {
        Ok(value) => value,
        Err(error) => return error,
    };

    match cli.command {
        GithubCommand::Repo => execute_repo(&context, globals),
        GithubCommand::Issues(args) => execute_issues(args, &context, globals),
        GithubCommand::Issue(args) => execute_issue(args, &context, globals),
        GithubCommand::Release(args) => execute_release(args, &context, globals),
        GithubCommand::Workflows => execute_workflows(&context, globals),
        GithubCommand::Workflow(args) => execute_workflow(args, &context, globals),
        GithubCommand::Runs(args) => execute_runs(args, &context, globals),
        GithubCommand::Run(args) => execute_run(args, &context, globals),
    }
}

fn execute_repo(context: &GithubContext, globals: &GlobalOptionsWire) -> InvocationResponse {
    let path = format!("/repos/{}/{}", context.repo.owner, context.repo.repo);
    let (html_url, default_branch, private) =
        match github_json::<GithubRepoResponse>(context, Method::GET, &path, None) {
            Ok(value) => (value.html_url, value.default_branch, value.private),
            Err(_) => (None, None, None),
        };

    let output = RepoOutput {
        command: "github.repo",
        repository: context.repo.full_name(),
        owner: context.repo.owner.clone(),
        name: context.repo.repo.clone(),
        remote_url: context.remote_url.clone(),
        api_url: context.api_url.clone(),
        html_url,
        default_branch,
        private,
    };

    render::render_success(
        globals,
        &output,
        format!(
            "{}\n",
            TextFormatter::stdout().paint(TextStyle::Key, &output.repository)
        ),
    )
}

fn execute_issues(
    args: IssuesArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let target = globals.limit.unwrap_or(20).clamp(1, 100);
    let issues = if let Some(search) = &args.search {
        let path = github_issue_search_path(context, &args, search, target);
        match github_json::<IssueSearchResponse>(context, Method::GET, &path, None) {
            Ok(value) => value.items.into_iter().take(target).collect(),
            Err(error) => return error,
        }
    } else {
        match list_github_issues(context, &args, target) {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
    let text = render_issues_text(&issues, TextFormatter::stdout());
    render::render_success(
        globals,
        &IssuesOutput {
            command: "github.issues",
            repository: context.repo.full_name(),
            state: args.state,
            labels: args.labels,
            assignee: args.assignee,
            author: args.author,
            since: args.since,
            search: args.search,
            issue_count: issues.len(),
            issues,
        },
        text,
    )
}

fn execute_issue(
    args: IssueArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        IssueCommand::View(args) => {
            let issue = match get_issue(context, args.number) {
                Ok(value) => value,
                Err(error) => return error,
            };
            render::render_success(
                globals,
                &IssueOutput {
                    command: "github.issue.view",
                    repository: context.repo.full_name(),
                    issue: issue.clone(),
                },
                render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
            )
        }
        IssueCommand::Create(args) => create_issue(context, args, globals),
        IssueCommand::Update(args) => update_issue(context, args, globals),
        IssueCommand::Close(args) => close_issue(context, args, globals),
        IssueCommand::Comment(args) => comment_issue(context, args, globals),
        IssueCommand::Comments(args) => issue_comments(context, args.number, globals),
    }
}

fn create_issue(
    context: &GithubContext,
    args: CreateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body_text = match resolve_optional_text(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), Value::String(args.title));
    if let Some(body_text) = body_text {
        body.insert("body".to_owned(), Value::String(body_text));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), json!(args.labels));
    }
    if !args.assignees.is_empty() {
        body.insert("assignees".to_owned(), json!(args.assignees));
    }
    let path = format!("/repos/{}/{}/issues", context.repo.owner, context.repo.repo);
    let issue =
        match github_json::<IssueResponse>(context, Method::POST, &path, Some(Value::Object(body)))
        {
            Ok(value) => value,
            Err(error) => return error,
        };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.create",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

fn update_issue(
    context: &GithubContext,
    args: UpdateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body_text = match resolve_optional_text(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut body = serde_json::Map::new();
    if let Some(title) = args.title {
        body.insert("title".to_owned(), Value::String(title));
    }
    if let Some(body_text) = body_text {
        body.insert("body".to_owned(), Value::String(body_text));
    }
    if let Some(state) = args.state {
        body.insert("state".to_owned(), Value::String(state));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), json!(args.labels));
    }
    if !args.assignees.is_empty() {
        body.insert("assignees".to_owned(), json!(args.assignees));
    }
    if body.is_empty() {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            "issue update requires at least one field",
        );
    }
    let path = format!(
        "/repos/{}/{}/issues/{}",
        context.repo.owner, context.repo.repo, args.number
    );
    let issue = match github_json::<IssueResponse>(
        context,
        Method::PATCH,
        &path,
        Some(Value::Object(body)),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.update",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

fn close_issue(
    context: &GithubContext,
    args: CloseIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let comment = match resolve_optional_text(args.comment, args.comment_file, "comment") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(comment) = comment
        && let Err(error) = create_issue_comment(context, args.number, comment)
    {
        return error;
    }
    let path = format!(
        "/repos/{}/{}/issues/{}",
        context.repo.owner, context.repo.repo, args.number
    );
    let issue = match github_json::<IssueResponse>(
        context,
        Method::PATCH,
        &path,
        Some(json!({ "state": "closed" })),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueOutput {
            command: "github.issue.close",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue), TextFormatter::stdout()),
    )
}

fn comment_issue(
    context: &GithubContext,
    args: CommentIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body = match resolve_required_text(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let comment = match create_issue_comment(context, args.number, body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueCommentOutput {
            command: "github.issue.comment",
            repository: context.repo.full_name(),
            number: args.number,
            comment: comment.clone(),
        },
        render_comments_text(std::slice::from_ref(&comment), TextFormatter::stdout()),
    )
}

fn issue_comments(
    context: &GithubContext,
    number: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = format!(
        "/repos/{}/{}/issues/{number}/comments?per_page={per_page}",
        context.repo.owner, context.repo.repo
    );
    let comments = match github_json::<Vec<IssueCommentResponse>>(context, Method::GET, &path, None)
    {
        Ok(value) => value,
        Err(error) => return error,
    };
    render::render_success(
        globals,
        &IssueCommentsOutput {
            command: "github.issue.comments",
            repository: context.repo.full_name(),
            number,
            comment_count: comments.len(),
            comments: comments.clone(),
        },
        render_comments_text(&comments, TextFormatter::stdout()),
    )
}

fn execute_release(
    args: ReleaseArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        ReleaseCommand::Get(args) => {
            let release = match get_release(context, &args.tag) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_release_text(&release, TextFormatter::stdout());
            render::render_success(
                globals,
                &ReleaseOutput {
                    command: "github.release.get",
                    repository: context.repo.full_name(),
                    release,
                },
                text,
            )
        }
        ReleaseCommand::Assets(args) => {
            let release = match get_release(context, &args.tag) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let assets = release.assets;
            let text = render_assets_text(&assets, TextFormatter::stdout());
            render::render_success(
                globals,
                &ReleaseAssetsOutput {
                    command: "github.release.assets",
                    repository: context.repo.full_name(),
                    tag: args.tag,
                    asset_count: assets.len(),
                    assets,
                },
                text,
            )
        }
        ReleaseCommand::Create(args) => create_release(context, args, globals),
    }
}

fn execute_workflows(context: &GithubContext, globals: &GlobalOptionsWire) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/workflows?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let workflows = match github_json::<WorkflowListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.workflows,
        Err(error) => return error,
    };
    let text = render_workflows_text(&workflows, TextFormatter::stdout());
    render::render_success(
        globals,
        &WorkflowsOutput {
            command: "github.workflows",
            repository: context.repo.full_name(),
            workflow_count: workflows.len(),
            workflows,
        },
        text,
    )
}

fn execute_workflow(
    args: WorkflowArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        WorkflowCommand::Run(args) => dispatch_workflow(context, args, globals),
    }
}

fn execute_runs(
    args: RunsArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(10).clamp(1, 100);
    let branch_query = args
        .branch
        .as_ref()
        .map(|branch| format!("&branch={}", urlencoding::encode(branch)))
        .unwrap_or_default();
    let path = if let Some(workflow) = &args.workflow {
        format!(
            "/repos/{}/{}/actions/workflows/{}/runs?per_page={per_page}{branch_query}",
            context.repo.owner,
            context.repo.repo,
            urlencoding::encode(workflow)
        )
    } else {
        format!(
            "/repos/{}/{}/actions/runs?per_page={per_page}{branch_query}",
            context.repo.owner, context.repo.repo
        )
    };

    let runs = match github_json::<RunsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.workflow_runs,
        Err(error) => return error,
    };
    let text = render_runs_text(&runs, TextFormatter::stdout());
    render::render_success(
        globals,
        &RunsOutput {
            command: "github.runs",
            repository: context.repo.full_name(),
            workflow: args.workflow,
            branch: args.branch,
            run_count: runs.len(),
            runs,
        },
        text,
    )
}

fn execute_run(
    args: RunArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        RunCommand::Get(args) => {
            let run = match get_run(context, args.run_id) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_runs_text(std::slice::from_ref(&run), TextFormatter::stdout());
            render::render_success(
                globals,
                &RunOutput {
                    command: "github.run.get",
                    repository: context.repo.full_name(),
                    run,
                },
                text,
            )
        }
        RunCommand::Wait(args) => wait_run(context, args, globals),
        RunCommand::Jobs(args) => run_jobs(context, args.run_id, globals),
        RunCommand::Logs(args) => {
            run_logs(context, args.run_id, args.grep, args.limits, globals, false)
        }
        RunCommand::Warnings(args) => {
            run_logs(context, args.run_id, None, args.limits, globals, true)
        }
        RunCommand::Artifacts(args) => run_artifacts(context, args.run_id, globals),
    }
}

fn create_release(
    context: &GithubContext,
    args: CreateReleaseArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let notes = match (args.notes, args.notes_file) {
        (Some(notes), None) => Some(notes),
        (None, Some(path)) => match fs::read_to_string(&path) {
            Ok(value) => Some(value),
            Err(error) => {
                return InvocationResponse::error(
                    "FILE_READ_FAILED",
                    format!("failed to read notes file '{path}': {error}"),
                );
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                "use either --notes or --notes-file, not both",
            );
        }
    };
    // An omitted option has to stay out of the payload: GitHub rejects an
    // explicit null with `nil is not a string` rather than falling back to its
    // own default.
    let mut body = serde_json::Map::new();
    body.insert("tag_name".to_owned(), Value::String(args.tag));
    body.insert("draft".to_owned(), Value::Bool(args.draft));
    body.insert("prerelease".to_owned(), Value::Bool(args.prerelease));
    for (key, value) in [
        ("target_commitish", args.target),
        ("name", args.title),
        ("body", notes),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), Value::String(value));
        }
    }
    let body = Value::Object(body);
    let path = format!(
        "/repos/{}/{}/releases",
        context.repo.owner, context.repo.repo
    );
    let release = match github_json::<ReleaseResponse>(context, Method::POST, &path, Some(body)) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_release_text(&release, TextFormatter::stdout());
    render::render_success(
        globals,
        &ReleaseOutput {
            command: "github.release.create",
            repository: context.repo.full_name(),
            release,
        },
        text,
    )
}

fn dispatch_workflow(
    context: &GithubContext,
    args: WorkflowRunArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let inputs = match parse_key_values(&args.inputs, "--input") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let body = json!({
        "ref": args.r#ref,
        "inputs": inputs,
    });
    let path = format!(
        "/repos/{}/{}/actions/workflows/{}/dispatches",
        context.repo.owner,
        context.repo.repo,
        urlencoding::encode(&args.workflow)
    );
    if let Err(error) = github_no_content(context, Method::POST, &path, Some(body)) {
        return error;
    }

    let formatter = TextFormatter::stdout();
    let text = format!(
        "{} {} {} {}\n",
        formatter.paint(TextStyle::Success, "dispatched"),
        formatter.paint(TextStyle::Key, &args.workflow),
        formatter.paint(TextStyle::Muted, "on"),
        formatter.paint(TextStyle::Key, &args.r#ref)
    );
    render::render_success(
        globals,
        &WorkflowDispatchOutput {
            command: "github.workflow.run",
            repository: context.repo.full_name(),
            workflow: args.workflow,
            r#ref: args.r#ref,
            input_count: inputs.as_object().map(|value| value.len()).unwrap_or(0),
            dispatched: true,
        },
        text,
    )
}

fn wait_run(
    context: &GithubContext,
    args: WaitRunArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let start = Instant::now();
    let timeout = Duration::from_secs(args.timeout_secs.max(1));
    let interval = Duration::from_secs(args.interval_secs.max(1));

    loop {
        let run = match get_run(context, args.run_id) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if run.status == "completed" {
            if args.fail_on_failure && run.conclusion.as_deref() != Some("success") {
                return InvocationResponse::error(
                    "GITHUB_RUN_FAILED",
                    format!(
                        "workflow run {} completed with conclusion {:?}",
                        run.id, run.conclusion
                    ),
                );
            }
            let elapsed_secs = start.elapsed().as_secs();
            let text = render_runs_text(std::slice::from_ref(&run), TextFormatter::stdout());
            return render::render_success(
                globals,
                &WaitRunOutput {
                    command: "github.run.wait",
                    repository: context.repo.full_name(),
                    run,
                    elapsed_secs,
                },
                text,
            );
        }

        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return InvocationResponse::error(
                "GITHUB_RUN_TIMEOUT",
                format!(
                    "workflow run {} did not complete within {} seconds",
                    args.run_id, args.timeout_secs
                ),
            );
        }

        let remaining = timeout - elapsed;
        if ah_plugin_api::cancellation::wait_or_cancel(interval.min(remaining)) {
            return InvocationResponse::error(
                "CANCELLED",
                format!("workflow run wait {} was cancelled", args.run_id),
            );
        }
    }
}

fn run_jobs(
    context: &GithubContext,
    run_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/jobs?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let jobs = match github_json::<JobsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.jobs,
        Err(error) => return error,
    };
    let text = render_jobs_text(&jobs, TextFormatter::stdout());
    render::render_success(
        globals,
        &JobsOutput {
            command: "github.run.jobs",
            repository: context.repo.full_name(),
            run_id,
            job_count: jobs.len(),
            jobs,
        },
        text,
    )
}

fn run_logs(
    context: &GithubContext,
    run_id: u64,
    grep: Option<String>,
    limits: LogLimitArgs,
    globals: &GlobalOptionsWire,
    warnings_only: bool,
) -> InvocationResponse {
    if limits.max_body_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-body-bytes must be >= 1");
    }
    if limits.max_expanded_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-expanded-bytes must be >= 1");
    }
    let (matches, truncated) = match download_run_logs(
        context,
        run_id,
        grep.as_deref(),
        warnings_only,
        globals.limit,
        limits.max_body_bytes,
        limits.max_expanded_bytes,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = matches
        .iter()
        .map(|line| format!("{}:{}: {}", line.file, line.line, line.text))
        .collect::<Vec<_>>()
        .join("\n")
        + if matches.is_empty() { "" } else { "\n" };
    render::render_success(
        globals,
        &LogsOutput {
            command: if warnings_only {
                "github.run.warnings"
            } else {
                "github.run.logs"
            },
            repository: context.repo.full_name(),
            run_id,
            grep,
            match_count: matches.len(),
            truncated,
            matches,
        },
        text,
    )
}

fn run_artifacts(
    context: &GithubContext,
    run_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/artifacts?per_page=100",
        context.repo.owner, context.repo.repo
    );
    let artifacts = match github_json::<ArtifactsListResponse>(context, Method::GET, &path, None) {
        Ok(value) => value.artifacts,
        Err(error) => return error,
    };
    let text = render_artifacts_text(&artifacts, TextFormatter::stdout());
    render::render_success(
        globals,
        &ArtifactsOutput {
            command: "github.run.artifacts",
            repository: context.repo.full_name(),
            run_id,
            artifact_count: artifacts.len(),
            artifacts,
        },
        text,
    )
}

fn get_issue(context: &GithubContext, number: u64) -> Result<IssueResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/issues/{number}",
        context.repo.owner, context.repo.repo
    );
    github_json::<IssueResponse>(context, Method::GET, &path, None)
}

fn create_issue_comment(
    context: &GithubContext,
    number: u64,
    body: String,
) -> Result<IssueCommentResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/issues/{number}/comments",
        context.repo.owner, context.repo.repo
    );
    github_json::<IssueCommentResponse>(context, Method::POST, &path, Some(json!({ "body": body })))
}

fn list_github_issues(
    context: &GithubContext,
    args: &IssuesArgs,
    target: usize,
) -> Result<Vec<IssueResponse>, InvocationResponse> {
    let per_page = target.min(100);
    let mut page = 1usize;
    let mut issues = Vec::with_capacity(target);

    while issues.len() < target {
        let path = github_issues_list_path(context, args, per_page, page);
        let page_items = github_json::<Vec<IssueResponse>>(context, Method::GET, &path, None)?;
        let page_len = page_items.len();
        issues.extend(
            page_items
                .into_iter()
                .filter(|issue| issue.pull_request.is_none())
                .take(target - issues.len()),
        );
        if page_len < per_page {
            break;
        }
        page = page.saturating_add(1);
    }

    Ok(issues)
}

fn github_issues_list_path(
    context: &GithubContext,
    args: &IssuesArgs,
    per_page: usize,
    page: usize,
) -> String {
    let mut query = vec![
        format!("state={}", urlencoding::encode(&args.state)),
        format!("per_page={per_page}"),
    ];
    if !args.labels.is_empty() {
        query.push(format!(
            "labels={}",
            urlencoding::encode(&args.labels.join(","))
        ));
    }
    if let Some(assignee) = &args.assignee {
        query.push(format!("assignee={}", urlencoding::encode(assignee)));
    }
    if let Some(author) = &args.author {
        query.push(format!("creator={}", urlencoding::encode(author)));
    }
    if let Some(since) = &args.since {
        query.push(format!("since={}", urlencoding::encode(since)));
    }
    let mut path = format!(
        "/repos/{}/{}/issues?{}",
        context.repo.owner,
        context.repo.repo,
        query.join("&")
    );
    if page > 1 {
        path.push_str(&format!("&page={page}"));
    }
    path
}

fn github_issue_search_path(
    context: &GithubContext,
    args: &IssuesArgs,
    search: &str,
    per_page: usize,
) -> String {
    let mut qualifiers = vec![
        format!("repo:{}/{}", context.repo.owner, context.repo.repo),
        "is:issue".to_owned(),
        search.to_owned(),
    ];
    if args.state != "all" {
        qualifiers.push(format!("state:{}", args.state));
    }
    for label in &args.labels {
        qualifiers.push(format!("label:\"{label}\""));
    }
    if let Some(assignee) = &args.assignee {
        qualifiers.push(format!("assignee:{assignee}"));
    }
    if let Some(author) = &args.author {
        qualifiers.push(format!("author:{author}"));
    }
    if let Some(since) = &args.since {
        qualifiers.push(format!("updated:>={since}"));
    }
    format!(
        "/search/issues?q={}&per_page={per_page}",
        urlencoding::encode(&qualifiers.join(" "))
    )
}

fn get_release(context: &GithubContext, tag: &str) -> Result<ReleaseResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/releases/tags/{}",
        context.repo.owner,
        context.repo.repo,
        urlencoding::encode(tag)
    );
    github_json::<ReleaseResponse>(context, Method::GET, &path, None)
}

fn get_run(
    context: &GithubContext,
    run_id: u64,
) -> Result<WorkflowRunResponse, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}",
        context.repo.owner, context.repo.repo
    );
    github_json::<WorkflowRunResponse>(context, Method::GET, &path, None)
}

fn github_context(args: &GithubConnectionArgs) -> Result<GithubContext, InvocationResponse> {
    let api_url = normalize_api_url(&args.api_url)?;
    let (repo, remote_url) = resolve_repo(args)?;
    let token = resolve_token(args, &api_url, remote_url.as_deref())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout_secs.max(1)))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "GITHUB_HTTP_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;

    Ok(GithubContext {
        client,
        api_url,
        token,
        repo,
        remote_url,
    })
}

fn resolve_repo(
    args: &GithubConnectionArgs,
) -> Result<(RepoSlug, Option<String>), InvocationResponse> {
    if let Some(repo) = &args.repo {
        return parse_repo_slug(repo)
            .map(|slug| (slug, None))
            .ok_or_else(|| invalid_repo(repo));
    }

    let remote_url = read_git_remote_url(&args.remote, args.cwd.as_deref())?;
    parse_github_remote_url(&remote_url)
        .map(|slug| (slug, Some(remote_url.clone())))
        .ok_or_else(|| {
            InvocationResponse::error(
                "GITHUB_REPO_UNDETECTED",
                format!(
                    "could not detect GitHub owner/repo from remote '{}': {}",
                    args.remote, remote_url
                ),
            )
        })
}

fn read_git_remote_url(remote: &str, cwd: Option<&Path>) -> Result<String, InvocationResponse> {
    let mut command = noninteractive_command("git");
    command.args(["remote", "get-url", remote]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|error| {
        InvocationResponse::error(
            "COMMAND_EXECUTION_FAILED",
            format!("failed to execute git remote get-url {remote}: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(InvocationResponse::error(
            "COMMAND_FAILED",
            format!(
                "git remote get-url {remote} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn parse_repo_slug(value: &str) -> Option<RepoSlug> {
    let normalized = value.trim().trim_end_matches(".git");
    let (owner, repo) = normalized.split_once('/')?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(RepoSlug {
        owner: owner.to_owned(),
        repo: repo.to_owned(),
    })
}

fn parse_github_remote_url(remote: &str) -> Option<RepoSlug> {
    let trimmed = remote.trim();
    if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("https://github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("http://github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("ssh://git@github.com/") {
        return parse_repo_slug(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("git+ssh://git@github.com/") {
        return parse_repo_slug(rest);
    }
    None
}

fn invalid_repo(value: &str) -> InvocationResponse {
    InvocationResponse::error(
        "INVALID_ARGUMENT",
        format!("--repo must use OWNER/REPO format, got '{value}'"),
    )
}

fn normalize_api_url(value: &str) -> Result<String, InvocationResponse> {
    let normalized = value.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--api-url must not be empty",
        ));
    }
    Ok(normalized)
}

fn resolve_token(
    args: &GithubConnectionArgs,
    api_url: &str,
    remote_url: Option<&str>,
) -> Result<Option<String>, InvocationResponse> {
    let explicit = args.token.clone().filter(|value| !value.trim().is_empty());
    credentials::resolve(api_url, explicit, &token_policy(args, api_url, remote_url))
        .map(|resolved| resolved.token)
        .map_err(|credentials::InsecureTokenTarget| insecure_token_target())
}

/// Where a GitHub token may come from when the caller supplied none.
fn token_policy<'a>(
    args: &GithubConnectionArgs,
    api_url: &str,
    remote_url: Option<&'a str>,
) -> credentials::TokenPolicy<'a> {
    credentials::TokenPolicy {
        // `api.github.com` and `github.com` are the same forge, so a credential
        // registered for either is registered for both.
        home_authorities: &[DEFAULT_API_AUTHORITY, "github.com"],
        env_vars: &["GITHUB_TOKEN", "GH_TOKEN"],
        credential_authority: args
            .use_git_credential
            .then(|| credential_authority(api_url))
            .flatten(),
        remote_url,
        helper_timeout: GIT_CREDENTIAL_TIMEOUT,
    }
}

fn insecure_token_target() -> InvocationResponse {
    InvocationResponse::error(
        "GITHUB_INSECURE_TOKEN_TARGET",
        "refusing to send a token to a cleartext --api-url; use https or a loopback host",
    )
}

/// Binds the credential helper lookup to the host that will receive the token,
/// so a redirected `--api-url` can never collect GitHub credentials.
fn credential_authority(api_url: &str) -> Option<String> {
    let authority = credentials::https_authority(api_url)?;
    Some(if authority == DEFAULT_API_AUTHORITY {
        "github.com".to_owned()
    } else {
        authority
    })
}

/// The GitHub REST API as this plugin talks to it.
fn api(context: &GithubContext) -> http::JsonApi<'_> {
    http::JsonApi {
        client: &context.client,
        base_url: &context.api_url,
        service: "GitHub",
        codes: http::ApiErrorCodes {
            transport: "GITHUB_HTTP_FAILED",
            status: "GITHUB_API_FAILED",
            decode: "GITHUB_RESPONSE_INVALID",
        },
        headers: &[
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
            ("User-Agent", "AIHelper-github-plugin"),
        ],
        // Every URL is built from `api_url`, which `resolve_token` already
        // accepted as the token's target, so there is nothing further to bind to.
        authorize: context.token.as_deref().map(|token| http::Authorization {
            scheme: http::AuthScheme::Bearer(token),
            authority: None,
        }),
        error_body_chars: 500,
    }
}

fn github_json<T>(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    api(context).json(method, path, body.as_ref())
}

fn github_no_content(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(), InvocationResponse> {
    api(context).send(method, path, body.as_ref()).map(drop)
}

fn github_response(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::blocking::Response, InvocationResponse> {
    api(context).send(method, path, body.as_ref())
}

fn download_run_logs(
    context: &GithubContext,
    run_id: u64,
    grep: Option<&str>,
    warnings_only: bool,
    line_limit: Option<usize>,
    max_body_bytes: usize,
    max_expanded_bytes: usize,
) -> Result<(Vec<LogLine>, bool), InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/logs",
        context.repo.owner, context.repo.repo
    );
    let response = github_response(context, Method::GET, &path, None)?;
    let bytes = read_bounded_log_body(response, run_id, max_body_bytes)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to open log archive for run {run_id}: {error}"),
        )
    })?;
    let max_lines = line_limit.unwrap_or(usize::MAX);
    let grep_lower = grep.map(str::to_lowercase);
    let mut matches = Vec::new();
    let mut expanded_bytes = 0usize;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| {
            InvocationResponse::error(
                "GITHUB_RESPONSE_INVALID",
                format!("failed to read log archive entry {index}: {error}"),
            )
        })?;
        if file.is_dir() {
            continue;
        }
        let file_name = file.name().to_owned();
        let mut reader = BufReader::new(file);
        let mut line_bytes = Vec::new();
        let mut line_number = 0usize;
        loop {
            line_bytes.clear();
            let remaining = max_expanded_bytes.saturating_sub(expanded_bytes);
            let read = reader
                .by_ref()
                .take(remaining.saturating_add(1) as u64)
                .read_until(b'\n', &mut line_bytes)
                .map_err(|error| {
                    InvocationResponse::error(
                        "GITHUB_RESPONSE_INVALID",
                        format!("failed to read log archive entry {index}: {error}"),
                    )
                })?;
            if read == 0 {
                break;
            }
            expanded_bytes = expanded_bytes.saturating_add(read);
            if expanded_bytes > max_expanded_bytes {
                return Err(InvocationResponse::error(
                    "GITHUB_RESPONSE_TOO_LARGE",
                    format!(
                        "expanded workflow logs exceed --max-expanded-bytes {max_expanded_bytes}"
                    ),
                ));
            }

            line_number += 1;
            while line_bytes
                .last()
                .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
            {
                line_bytes.pop();
            }
            let Ok(line) = std::str::from_utf8(&line_bytes) else {
                continue;
            };
            let text = render::strip_ansi_sequences(line);
            let selected = if warnings_only {
                is_warning_like(&text)
            } else if let Some(needle) = &grep_lower {
                text.to_lowercase().contains(needle)
            } else {
                true
            };
            if !selected {
                continue;
            }
            if matches.len() == max_lines {
                return Ok((matches, true));
            }
            matches.push(LogLine {
                file: file_name.clone(),
                line: line_number,
                text,
            });
        }
    }
    Ok((matches, false))
}

fn read_bounded_log_body(
    mut response: reqwest::blocking::Response,
    run_id: u64,
    max_body_bytes: usize,
) -> Result<Vec<u8>, InvocationResponse> {
    if response
        .content_length()
        .is_some_and(|length| length > max_body_bytes as u64)
    {
        return Err(InvocationResponse::error(
            "GITHUB_RESPONSE_TOO_LARGE",
            format!("workflow log archive exceeds --max-body-bytes {max_body_bytes}"),
        ));
    }

    let mut bytes = Vec::with_capacity(max_body_bytes.min(64 * 1024));
    response
        .by_ref()
        .take(max_body_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            InvocationResponse::error(
                "GITHUB_RESPONSE_INVALID",
                format!("failed to read log archive for run {run_id}: {error}"),
            )
        })?;
    if bytes.len() > max_body_bytes {
        return Err(InvocationResponse::error(
            "GITHUB_RESPONSE_TOO_LARGE",
            format!("workflow log archive exceeds --max-body-bytes {max_body_bytes}"),
        ));
    }
    Ok(bytes)
}

fn parse_key_values(values: &[String], flag_name: &str) -> Result<Value, InvocationResponse> {
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

fn resolve_optional_text(
    inline: Option<String>,
    file: Option<String>,
    field_name: &str,
) -> Result<Option<String>, InvocationResponse> {
    match (inline, file) {
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => fs::read_to_string(&path).map(Some).map_err(|error| {
            InvocationResponse::error(
                "FILE_READ_FAILED",
                format!("failed to read {field_name} file '{path}': {error}"),
            )
        }),
        (None, None) => Ok(None),
        (Some(_), Some(_)) => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("use either --{field_name} or --{field_name}-file, not both"),
        )),
    }
}

fn resolve_required_text(
    inline: Option<String>,
    file: Option<String>,
    field_name: &str,
) -> Result<String, InvocationResponse> {
    match resolve_optional_text(inline, file, field_name)? {
        Some(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!("--{field_name} or --{field_name}-file is required"),
        )),
    }
}

fn is_warning_like(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("warning")
        || lower.contains("deprecated")
        || lower.contains("deprecation")
        || lower.contains("will be removed")
}

fn render_issues_text(issues: &[IssueResponse], formatter: TextFormatter) -> String {
    if issues.is_empty() {
        return String::new();
    }
    issues
        .iter()
        .map(|issue| {
            format!(
                "#{} {} {} {}",
                formatter.paint(TextStyle::Key, issue.number),
                formatter.paint(issue_state_style(&issue.state), &issue.state),
                issue.title,
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    issue.html_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_comments_text(comments: &[IssueCommentResponse], formatter: TextFormatter) -> String {
    if comments.is_empty() {
        return String::new();
    }
    comments
        .iter()
        .map(|comment| {
            let first_line = comment
                .body
                .as_deref()
                .unwrap_or("")
                .lines()
                .next()
                .unwrap_or("");
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, comment.id),
                formatter.paint(
                    TextStyle::Key,
                    comment
                        .user
                        .as_ref()
                        .map(|user| user.login.as_str())
                        .unwrap_or("-")
                ),
                first_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_release_text(release: &ReleaseResponse, formatter: TextFormatter) -> String {
    format!(
        "{} draft={} prerelease={} assets={} {}\n",
        formatter.paint(TextStyle::Key, &release.tag_name),
        formatter.paint(bool_warning_style(release.draft), release.draft),
        formatter.paint(bool_warning_style(release.prerelease), release.prerelease),
        formatter.paint(TextStyle::Muted, release.assets.len()),
        render::paint_if_present(
            formatter,
            TextStyle::Key,
            release.html_url.as_deref().unwrap_or("")
        )
    )
}

fn render_assets_text(assets: &[ReleaseAsset], formatter: TextFormatter) -> String {
    if assets.is_empty() {
        return String::new();
    }
    assets
        .iter()
        .map(|asset| {
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, &asset.name),
                formatter.paint(TextStyle::Muted, asset.size),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    asset.browser_download_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_workflows_text(workflows: &[WorkflowResponse], formatter: TextFormatter) -> String {
    if workflows.is_empty() {
        return String::new();
    }
    workflows
        .iter()
        .map(|workflow| {
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, workflow.id),
                formatter.paint(workflow_state_style(&workflow.state), &workflow.state),
                formatter.paint(TextStyle::Key, &workflow.path)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_runs_text(runs: &[WorkflowRunResponse], formatter: TextFormatter) -> String {
    if runs.is_empty() {
        return String::new();
    }
    runs.iter()
        .map(|run| {
            let conclusion = run.conclusion.as_deref().unwrap_or("-");
            format!(
                "{} {} {} {} {} {}",
                formatter.paint(TextStyle::Key, run.id),
                run.name.as_deref().unwrap_or("-"),
                formatter.paint(TextStyle::Muted, &run.event),
                formatter.paint(execution_status_style(&run.status), &run.status),
                formatter.paint(conclusion_style(conclusion), conclusion),
                render::paint_if_present(
                    formatter,
                    TextStyle::Key,
                    run.html_url.as_deref().unwrap_or("")
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_jobs_text(jobs: &[JobResponse], formatter: TextFormatter) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    jobs.iter()
        .map(|job| {
            let conclusion = job.conclusion.as_deref().unwrap_or("-");
            format!(
                "{} {} {}",
                formatter.paint(TextStyle::Key, &job.name),
                formatter.paint(execution_status_style(&job.status), &job.status),
                formatter.paint(conclusion_style(conclusion), conclusion)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_artifacts_text(artifacts: &[ArtifactResponse], formatter: TextFormatter) -> String {
    if artifacts.is_empty() {
        return String::new();
    }
    artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} {} expired={}",
                formatter.paint(TextStyle::Key, &artifact.name),
                formatter.paint(TextStyle::Muted, artifact.size_in_bytes),
                formatter.paint(
                    if artifact.expired {
                        TextStyle::Error
                    } else {
                        TextStyle::Success
                    },
                    artifact.expired
                )
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn issue_state_style(state: &str) -> TextStyle {
    match state {
        "open" => TextStyle::Success,
        "closed" => TextStyle::Muted,
        _ => TextStyle::Warning,
    }
}

fn workflow_state_style(state: &str) -> TextStyle {
    match state {
        "active" => TextStyle::Success,
        value if value.starts_with("disabled") => TextStyle::Warning,
        _ => TextStyle::Muted,
    }
}

fn execution_status_style(status: &str) -> TextStyle {
    match status {
        "queued" | "pending" | "in_progress" | "requested" | "waiting" => TextStyle::Warning,
        "success" | "active" => TextStyle::Success,
        "failure" | "failed" | "cancelled" | "timed_out" | "action_required" => TextStyle::Error,
        _ => TextStyle::Muted,
    }
}

fn conclusion_style(conclusion: &str) -> TextStyle {
    match conclusion {
        "success" => TextStyle::Success,
        "failure" | "cancelled" | "timed_out" | "action_required" | "startup_failure" => {
            TextStyle::Error
        }
        "neutral" | "skipped" | "-" => TextStyle::Muted,
        _ => TextStyle::Warning,
    }
}

fn bool_warning_style(value: bool) -> TextStyle {
    if value {
        TextStyle::Warning
    } else {
        TextStyle::Muted
    }
}

fn plugin_manual() -> PluginManual {
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
    use std::{io::Write, process::Stdio};

    use std::{
        collections::HashMap,
        io::{BufRead, BufReader},
        net::{TcpListener, TcpStream},
        process::Command,
        sync::{Arc, Mutex},
    };

    use clap::{CommandFactory, Parser};

    use super::*;

    #[test]
    fn issue_renderer_preserves_plain_contract_and_styles_metadata() {
        let issue = IssueResponse {
            number: 42,
            title: "Fix formatter".to_owned(),
            body: Some("raw body".to_owned()),
            state: "open".to_owned(),
            html_url: Some("https://github.com/acme/tool/issues/42".to_owned()),
            user: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            comments: Some(0),
            created_at: None,
            updated_at: None,
            closed_at: None,
            pull_request: None,
        };

        assert_eq!(
            render_issues_text(
                std::slice::from_ref(&issue),
                TextFormatter::with_color(false)
            ),
            "#42 open Fix formatter https://github.com/acme/tool/issues/42\n"
        );

        let rendered = render_issues_text(&[issue], TextFormatter::with_color(true));
        assert!(rendered.contains("#\u{1b}[36m42\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[32mopen\u{1b}[0m"));
        assert!(rendered.contains("Fix formatter"));
        assert!(!rendered.contains("\u{1b}[0mFix formatter"));
    }

    #[test]
    fn workflow_renderers_map_execution_states() {
        let run = WorkflowRunResponse {
            id: 7,
            name: Some("CI".to_owned()),
            event: "push".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("failure".to_owned()),
            head_branch: Some("main".to_owned()),
            head_sha: "abc123".to_owned(),
            html_url: Some("https://github.com/acme/tool/actions/runs/7".to_owned()),
            created_at: None,
            updated_at: None,
        };

        assert_eq!(
            render_runs_text(std::slice::from_ref(&run), TextFormatter::with_color(false)),
            "7 CI push completed failure https://github.com/acme/tool/actions/runs/7\n"
        );

        let rendered = render_runs_text(&[run], TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36m7\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[2mcompleted\u{1b}[0m"));
        assert!(rendered.contains("\u{1b}[1;31mfailure\u{1b}[0m"));
    }

    #[test]
    fn artifact_renderer_styles_expiration_without_changing_plain_shape() {
        let artifact = ArtifactResponse {
            id: 8,
            name: "ah-windows.zip".to_owned(),
            size_in_bytes: 123,
            expired: true,
            archive_download_url: None,
        };

        assert_eq!(
            render_artifacts_text(
                std::slice::from_ref(&artifact),
                TextFormatter::with_color(false)
            ),
            "ah-windows.zip 123 expired=true\n"
        );

        let rendered = render_artifacts_text(&[artifact], TextFormatter::with_color(true));
        assert!(rendered.contains("\u{1b}[36mah-windows.zip\u{1b}[0m"));
        assert!(rendered.contains("expired=\u{1b}[1;31mtrue\u{1b}[0m"));
    }

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
    fn ambient_tokens_reach_only_github_loopback_or_the_detected_remote() {
        // Asserted through the policy the plugin actually builds, not a
        // restatement of it.
        let accepts = |authority: &str, remote: Option<&str>| {
            token_policy(&connection_args(DEFAULT_API_URL), DEFAULT_API_URL, remote)
                .accepts_ambient_credentials(authority)
        };
        let remote = Some("git@ghe.corp.example:owner/repo.git");

        assert!(accepts("api.github.com", None));
        assert!(accepts("github.com", None));
        assert!(accepts("127.0.0.1:8080", None));
        assert!(accepts("ghe.corp.example", remote));
        assert!(accepts(
            "ghe.corp.example",
            Some("https://ghe.corp.example/owner/repo.git")
        ));

        // A redirected --api-url is never trusted with an ambient credential.
        assert!(!accepts("attacker.example", None));
        assert!(!accepts("attacker.example", remote));
        assert!(!accepts("ghe.corp.example", None));
    }

    #[test]
    fn tokens_travel_only_over_https_or_loopback() {
        assert_eq!(
            credentials::secure_authority("https://ghe.corp.example/api/v3").as_deref(),
            Some("ghe.corp.example")
        );
        assert_eq!(
            credentials::secure_authority("http://127.0.0.1:8080").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(
            credentials::secure_authority("http://localhost:8080").as_deref(),
            Some("localhost:8080")
        );
        assert_eq!(
            credentials::secure_authority("http://ghe.corp.example"),
            None
        );
    }

    #[test]
    fn explicit_token_to_a_cleartext_host_is_refused() {
        let mut args = connection_args("http://ghe.corp.example");
        args.token = Some("explicit-token".to_owned());

        let error = resolve_token(&args, &args.api_url.clone(), None)
            .expect_err("a cleartext destination must not receive a token");

        assert_eq!(
            error.error_code.as_deref(),
            Some("GITHUB_INSECURE_TOKEN_TARGET")
        );
        assert!(!format!("{error:?}").contains("explicit-token"));
    }

    #[test]
    fn explicit_token_still_reaches_a_caller_chosen_https_host() {
        // The caller supplied both the credential and the destination, so only
        // the cleartext rule applies to it.
        let mut args = connection_args("https://ghe.corp.example/api/v3");
        args.token = Some("explicit-token".to_owned());

        let token = resolve_token(&args, &args.api_url.clone(), None).unwrap();

        assert_eq!(token.as_deref(), Some("explicit-token"));
    }

    fn connection_args(api_url: &str) -> GithubConnectionArgs {
        GithubConnectionArgs {
            repo: None,
            remote: DEFAULT_REMOTE.to_owned(),
            api_url: api_url.to_owned(),
            token: None,
            use_git_credential: false,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cwd: None,
        }
    }

    #[test]
    fn credential_lookup_is_bound_to_the_api_url_host() {
        assert_eq!(
            credential_authority("https://api.github.com").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            credential_authority("https://ghe.corp.example/api/v3").as_deref(),
            Some("ghe.corp.example")
        );
        for redirected in [
            "https://attacker.example/api/v3",
            "https://api.github.com@attacker.example/api/v3",
        ] {
            assert_ne!(
                credential_authority(redirected).as_deref(),
                Some("github.com"),
                "{redirected}"
            );
        }
        assert_eq!(credential_authority("http://api.github.com"), None);
        assert_eq!(credential_authority("https://"), None);
    }

    #[test]
    fn credential_helper_timeout_kills_the_child() {
        if std::env::var_os("AH_GITHUB_TEST_CREDENTIAL_SLEEP").is_some() {
            thread::sleep(Duration::from_secs(1));
            return;
        }
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::credential_helper_timeout_kills_the_child",
            ])
            .env("AH_GITHUB_TEST_CREDENTIAL_SLEEP", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        assert!(credentials::wait_for_credential_child(child, Duration::from_millis(20)).is_none());
    }

    #[test]
    fn parses_common_github_remotes() {
        assert_eq!(
            parse_github_remote_url("https://github.com/Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
        assert_eq!(
            parse_github_remote_url("git@github.com:Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
        assert_eq!(
            parse_github_remote_url("ssh://git@github.com/Bobsans/AIHelper.git")
                .expect("repo should parse")
                .full_name(),
            "Bobsans/AIHelper"
        );
    }

    #[test]
    fn repo_command_falls_back_when_api_lookup_fails() {
        let server = MockServer::new(vec![MockResponse::json(
            500,
            r#"{"message":"server error"}"#,
        )]);

        let response = invoke_json(&["--repo", "acme/tool", "--api-url", &server.url(), "repo"]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.repo");
        assert_eq!(payload["repository"], "acme/tool");
        assert!(payload["html_url"].is_null());
        assert!(payload["default_branch"].is_null());
        assert!(payload["private"].is_null());
    }

    #[test]
    fn rejects_non_github_remote() {
        assert!(parse_github_remote_url("https://gitlab.com/Bobsans/AIHelper.git").is_none());
    }

    #[test]
    fn detects_warning_like_lines() {
        assert!(is_warning_like("Node.js 20 actions are deprecated."));
        assert!(is_warning_like("warning: output truncated"));
        assert!(!is_warning_like("build completed successfully"));
    }

    #[test]
    fn strips_ansi_sequences() {
        assert_eq!(
            render::strip_ansi_sequences("\u{1b}[1mDownloaded\u{1b}[0m"),
            "Downloaded"
        );
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

    #[test]
    fn issues_list_uses_filters_and_limit() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"[{
                "number": 12,
                "title": "Fix build",
                "body": "body",
                "state": "open",
                "html_url": "https://github.com/acme/tool/issues/12",
                "user": {"login": "alice"},
                "labels": [{"name": "bug"}],
                "assignees": [{"login": "bob"}],
                "comments": 1,
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z",
                "closed_at": null
            }]"#,
        )]);

        let response = invoke_json_with_limit(
            &[
                "--repo",
                "acme/tool",
                "--api-url",
                &server.url(),
                "issues",
                "--state",
                "all",
                "--label",
                "bug",
                "--assignee",
                "bob",
                "--author",
                "alice",
                "--since",
                "2026-05-07T00:00:00Z",
            ],
            Some(5),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.issues");
        assert_eq!(payload["issue_count"], 1);
        let request = only_request(&server);
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.path,
            "/repos/acme/tool/issues?state=all&per_page=5&labels=bug&assignee=bob&creator=alice&since=2026-05-07T00%3A00%3A00Z"
        );
    }

    #[test]
    fn issues_list_pages_past_pull_requests() {
        let first_page = format!(
            "[{},{}]",
            pull_request_issue_json(1),
            pull_request_issue_json(2)
        );
        let second_page = format!("[{},{}]", issue_json(3, "open"), issue_json(4, "open"));
        let server = MockServer::new(vec![
            MockResponse::json(200, &first_page),
            MockResponse::json(200, &second_page),
        ]);

        let response = invoke_json_with_limit(
            &["--repo", "acme/tool", "--api-url", &server.url(), "issues"],
            Some(2),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["issue_count"], 2);
        assert_eq!(payload["issues"][0]["number"], 3);
        assert_eq!(payload["issues"][1]["number"], 4);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].path,
            "/repos/acme/tool/issues?state=open&per_page=2"
        );
        assert_eq!(
            requests[1].path,
            "/repos/acme/tool/issues?state=open&per_page=2&page=2"
        );
    }

    #[test]
    fn issues_search_uses_search_api() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{"items": [{
                "number": 13,
                "title": "Crash on startup",
                "body": "body",
                "state": "open",
                "html_url": "https://github.com/acme/tool/issues/13",
                "user": {"login": "alice"},
                "labels": [],
                "assignees": [],
                "comments": 0,
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z",
                "closed_at": null
            }]}"#,
        )]);

        let response = invoke_json_with_limit(
            &[
                "--repo",
                "acme/tool",
                "--api-url",
                &server.url(),
                "issues",
                "--search",
                "startup crash",
            ],
            Some(3),
        );

        assert!(response.success, "{response:?}");
        let request = only_request(&server);
        assert!(request.path.starts_with("/search/issues?q="));
        assert!(request.path.contains("per_page=3"));
    }

    #[test]
    fn issue_view_uses_expected_request() {
        let server = MockServer::new(vec![MockResponse::json(200, &issue_json(21, "open"))]);
        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "issue",
            "view",
            "21",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.issue.view");
        assert_eq!(payload["issue"]["number"], 21);
        let request = only_request(&server);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/repos/acme/tool/issues/21");
    }

    #[test]
    fn issue_create_and_update_send_expected_bodies() {
        let create_server = MockServer::new(vec![MockResponse::json(201, &issue_json(21, "open"))]);
        let create_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &create_server.url(),
            "issue",
            "create",
            "--title",
            "Fix build",
            "--body",
            "details",
            "--label",
            "bug",
            "--assignee",
            "bob",
        ]);
        assert!(create_response.success, "{create_response:?}");
        let create_request = only_request(&create_server);
        assert_eq!(create_request.method, "POST");
        assert_eq!(create_request.path, "/repos/acme/tool/issues");
        let create_body: Value =
            serde_json::from_str(&create_request.body).expect("body should be json");
        assert_eq!(create_body["title"], "Fix build");
        assert_eq!(create_body["body"], "details");
        assert_eq!(create_body["labels"][0], "bug");
        assert_eq!(create_body["assignees"][0], "bob");

        let update_server =
            MockServer::new(vec![MockResponse::json(200, &issue_json(21, "closed"))]);
        let update_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &update_server.url(),
            "issue",
            "update",
            "21",
            "--state",
            "closed",
            "--label",
            "fixed",
        ]);
        assert!(update_response.success, "{update_response:?}");
        let update_request = only_request(&update_server);
        assert_eq!(update_request.method, "PATCH");
        assert_eq!(update_request.path, "/repos/acme/tool/issues/21");
        let update_body: Value =
            serde_json::from_str(&update_request.body).expect("body should be json");
        assert_eq!(update_body["state"], "closed");
        assert_eq!(update_body["labels"][0], "fixed");
    }

    #[test]
    fn issue_close_comments_then_closes() {
        let server = MockServer::new(vec![
            MockResponse::json(201, &issue_comment_json(101)),
            MockResponse::json(200, &issue_json(21, "closed")),
        ]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "issue",
            "close",
            "21",
            "--comment",
            "fixed",
        ]);

        assert!(response.success, "{response:?}");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/repos/acme/tool/issues/21/comments");
        assert_eq!(requests[1].method, "PATCH");
        assert_eq!(requests[1].path, "/repos/acme/tool/issues/21");
    }

    #[test]
    fn issue_comment_and_comments_work() {
        let comment_server =
            MockServer::new(vec![MockResponse::json(201, &issue_comment_json(101))]);
        let comment_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &comment_server.url(),
            "issue",
            "comment",
            "21",
            "--body",
            "hello",
        ]);
        assert!(comment_response.success, "{comment_response:?}");
        let comment_payload = response_json(&comment_response);
        assert_eq!(comment_payload["command"], "github.issue.comment");

        let list_server = MockServer::new(vec![MockResponse::json(
            200,
            &format!("[{}]", issue_comment_json(101)),
        )]);
        let list_response = invoke_json_with_limit(
            &[
                "--repo",
                "acme/tool",
                "--api-url",
                &list_server.url(),
                "issue",
                "comments",
                "21",
            ],
            Some(2),
        );
        assert!(list_response.success, "{list_response:?}");
        let list_payload = response_json(&list_response);
        assert_eq!(list_payload["command"], "github.issue.comments");
        assert_eq!(list_payload["comment_count"], 1);
        assert_eq!(
            only_request(&list_server).path,
            "/repos/acme/tool/issues/21/comments?per_page=2"
        );
    }

    #[test]
    fn release_get_uses_expected_request_and_auth_header() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "id": 10,
                "tag_name": "v1.0.0",
                "name": "v1.0.0",
                "draft": false,
                "prerelease": false,
                "html_url": "https://github.com/acme/tool/releases/tag/v1.0.0",
                "published_at": "2026-05-06T00:00:00Z",
                "assets": []
            }"#,
        )]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "--token",
            "secret-token",
            "release",
            "get",
            "v1.0.0",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.release.get");
        assert_eq!(payload["repository"], "acme/tool");
        assert_eq!(payload["release"]["tag_name"], "v1.0.0");

        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/repos/acme/tool/releases/tags/v1.0.0");
        assert_eq!(
            requests[0].header("authorization"),
            Some("Bearer secret-token")
        );
        assert_eq!(
            requests[0].header("x-github-api-version"),
            Some("2022-11-28")
        );
    }

    #[test]
    fn release_assets_returns_asset_list() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "id": 10,
                "tag_name": "v1.0.0",
                "name": "v1.0.0",
                "draft": false,
                "prerelease": false,
                "html_url": "https://github.com/acme/tool/releases/tag/v1.0.0",
                "published_at": "2026-05-06T00:00:00Z",
                "assets": [
                    {
                        "id": 1,
                        "name": "tool-linux.zip",
                        "size": 123,
                        "browser_download_url": "https://example.test/tool-linux.zip"
                    }
                ]
            }"#,
        )]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "release",
            "assets",
            "v1.0.0",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.release.assets");
        assert_eq!(payload["asset_count"], 1);
        assert_eq!(payload["assets"][0]["name"], "tool-linux.zip");
    }

    #[test]
    fn release_create_posts_expected_body() {
        let server = MockServer::new(vec![MockResponse::json(
            201,
            r#"{
                "id": 11,
                "tag_name": "v1.0.1",
                "name": "v1.0.1",
                "draft": true,
                "prerelease": false,
                "html_url": "https://github.com/acme/tool/releases/tag/v1.0.1",
                "published_at": null,
                "assets": []
            }"#,
        )]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "release",
            "create",
            "v1.0.1",
            "--title",
            "v1.0.1",
            "--notes",
            "release notes",
            "--target",
            "main",
            "--draft",
        ]);

        assert!(response.success, "{response:?}");
        let request = only_request(&server);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/repos/acme/tool/releases");
        let body: Value = serde_json::from_str(&request.body).expect("body should be json");
        assert_eq!(body["tag_name"], "v1.0.1");
        assert_eq!(body["target_commitish"], "main");
        assert_eq!(body["name"], "v1.0.1");
        assert_eq!(body["body"], "release notes");
        assert_eq!(body["draft"], true);
        assert_eq!(body["prerelease"], false);
    }

    #[test]
    fn release_create_omits_options_that_were_not_given() {
        let server = MockServer::new(vec![MockResponse::json(
            201,
            r#"{
                "id": 11,
                "tag_name": "v1.0.1",
                "name": null,
                "draft": false,
                "prerelease": false,
                "html_url": "https://github.com/acme/tool/releases/tag/v1.0.1",
                "published_at": null,
                "assets": []
            }"#,
        )]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "release",
            "create",
            "v1.0.1",
        ]);

        assert!(response.success, "{response:?}");
        let body: Value =
            serde_json::from_str(&only_request(&server).body).expect("body should be json");
        assert_eq!(body["tag_name"], "v1.0.1");
        for key in ["target_commitish", "name", "body"] {
            assert!(body.get(key).is_none(), "{key} should be omitted: {body}");
        }
    }

    #[test]
    fn workflow_dispatch_posts_ref_and_inputs() {
        let server = MockServer::new(vec![MockResponse::empty(204)]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "workflow",
            "run",
            "release.yml",
            "--ref",
            "main",
            "--input",
            "dry_run=true",
        ]);

        assert!(response.success, "{response:?}");
        let request = only_request(&server);
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.path,
            "/repos/acme/tool/actions/workflows/release.yml/dispatches"
        );
        let body: Value = serde_json::from_str(&request.body).expect("body should be json");
        assert_eq!(body["ref"], "main");
        assert_eq!(body["inputs"]["dry_run"], "true");
    }

    #[test]
    fn runs_command_includes_workflow_branch_and_limit() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "workflow_runs": [
                    {
                        "id": 42,
                        "name": "CI",
                        "event": "push",
                        "status": "completed",
                        "conclusion": "success",
                        "head_branch": "main",
                        "head_sha": "abc123",
                        "html_url": "https://github.com/acme/tool/actions/runs/42",
                        "created_at": "2026-05-06T00:00:00Z",
                        "updated_at": "2026-05-06T00:01:00Z"
                    }
                ]
            }"#,
        )]);

        let response = invoke_json_with_limit(
            &[
                "--repo",
                "acme/tool",
                "--api-url",
                &server.url(),
                "runs",
                "--workflow",
                "ci.yml",
                "--branch",
                "main",
            ],
            Some(3),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["run_count"], 1);
        let request = only_request(&server);
        assert_eq!(
            request.path,
            "/repos/acme/tool/actions/workflows/ci.yml/runs?per_page=3&branch=main"
        );
    }

    #[test]
    fn run_wait_polls_until_completed() {
        let server = MockServer::new(vec![
            MockResponse::json(200, &workflow_run_json(42, "in_progress", None)),
            MockResponse::json(200, &workflow_run_json(42, "completed", Some("success"))),
        ]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "run",
            "wait",
            "42",
            "--interval-secs",
            "1",
            "--timeout-secs",
            "5",
            "--fail-on-failure",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.run.wait");
        assert_eq!(payload["run"]["status"], "completed");
        assert_eq!(payload["run"]["conclusion"], "success");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn run_jobs_and_artifacts_decode_lists() {
        let jobs_server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "jobs": [
                    {
                        "id": 7,
                        "name": "test",
                        "status": "completed",
                        "conclusion": "success",
                        "html_url": "https://github.com/acme/tool/actions/jobs/7",
                        "started_at": "2026-05-06T00:00:00Z",
                        "completed_at": "2026-05-06T00:01:00Z"
                    }
                ]
            }"#,
        )]);
        let jobs_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &jobs_server.url(),
            "run",
            "jobs",
            "42",
        ]);
        assert!(jobs_response.success, "{jobs_response:?}");
        let jobs_payload = response_json(&jobs_response);
        assert_eq!(jobs_payload["job_count"], 1);
        assert_eq!(jobs_payload["jobs"][0]["name"], "test");

        let artifacts_server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "artifacts": [
                    {
                        "id": 8,
                        "name": "ah-linux-x64.zip",
                        "size_in_bytes": 123,
                        "expired": false,
                        "archive_download_url": "https://api.github.com/artifacts/8/zip"
                    }
                ]
            }"#,
        )]);
        let artifacts_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &artifacts_server.url(),
            "run",
            "artifacts",
            "42",
        ]);
        assert!(artifacts_response.success, "{artifacts_response:?}");
        let artifacts_payload = response_json(&artifacts_response);
        assert_eq!(artifacts_payload["artifact_count"], 1);
        assert_eq!(
            artifacts_payload["artifacts"][0]["name"],
            "ah-linux-x64.zip"
        );
    }

    #[test]
    fn run_logs_and_warnings_read_zip_archive() {
        let zip_bytes = log_zip_bytes(&[(
            "Build/1_step.txt",
            "normal line\nNode.js 20 actions are deprecated\n\u{1b}[1mwarning: noisy\u{1b}[0m\n",
        )]);
        let server = MockServer::new(vec![MockResponse::bytes(200, "application/zip", zip_bytes)]);

        let response = invoke_json_with_limit(
            &[
                "--repo",
                "acme/tool",
                "--api-url",
                &server.url(),
                "run",
                "warnings",
                "42",
            ],
            Some(10),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "github.run.warnings");
        assert_eq!(payload["match_count"], 2);
        assert_eq!(
            payload["matches"][1]["text"], "warning: noisy",
            "ANSI escape sequences should be stripped"
        );
    }

    #[test]
    fn run_logs_rejects_compressed_and_expanded_overflow() {
        let zip_bytes = log_zip_bytes(&[("Build/step.txt", "0123456789abcdef\n")]);
        let compressed_limit = zip_bytes.len().saturating_sub(1).to_string();
        let compressed_server = MockServer::new(vec![MockResponse::bytes(
            200,
            "application/zip",
            zip_bytes.clone(),
        )]);
        let compressed_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &compressed_server.url(),
            "run",
            "logs",
            "42",
            "--max-body-bytes",
            &compressed_limit,
        ]);
        assert_eq!(
            compressed_response.error_code.as_deref(),
            Some("GITHUB_RESPONSE_TOO_LARGE")
        );

        let expanded_server =
            MockServer::new(vec![MockResponse::bytes(200, "application/zip", zip_bytes)]);
        let expanded_response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &expanded_server.url(),
            "run",
            "logs",
            "42",
            "--max-expanded-bytes",
            "8",
        ]);
        assert_eq!(
            expanded_response.error_code.as_deref(),
            Some("GITHUB_RESPONSE_TOO_LARGE")
        );
    }

    #[test]
    fn github_api_failure_has_stable_error_code() {
        let server = MockServer::new(vec![MockResponse::json(
            404,
            r#"{"message":"Not Found","status":"404"}"#,
        )]);

        let response = invoke_json(&[
            "--repo",
            "acme/tool",
            "--api-url",
            &server.url(),
            "release",
            "get",
            "missing",
        ]);

        assert!(!response.success);
        assert_eq!(response.error_code.as_deref(), Some("GITHUB_API_FAILED"));
        assert!(
            response
                .error_message
                .as_deref()
                .unwrap_or("")
                .contains("HTTP 404")
        );
    }

    fn invoke_json(argv: &[&str]) -> InvocationResponse {
        invoke_json_with_limit(argv, None)
    }

    fn invoke_json_with_limit(argv: &[&str], limit: Option<usize>) -> InvocationResponse {
        let mut argv = argv
            .iter()
            .map(|item| (*item).to_owned())
            .collect::<Vec<_>>();
        if !argv.iter().any(|item| item == "--token") {
            argv.splice(0..0, ["--token".to_owned(), "test-token".to_owned()]);
        }
        let request = InvocationRequest {
            resolved_secrets: Default::default(),
            domain: DOMAIN.to_owned(),
            argv,
            globals: GlobalOptionsWire {
                json: true,
                quiet: false,
                limit,
                cwd: None,
            },
        };
        let request_json = serde_json::to_string(&request).expect("request should serialize");
        let request_c = std::ffi::CString::new(request_json).expect("request should be cstring");
        invoke_from_raw(request_c.as_ptr())
    }

    fn response_json(response: &InvocationResponse) -> Value {
        serde_json::from_str(response.message.as_deref().expect("message should exist"))
            .expect("message should be json")
    }

    fn workflow_run_json(id: u64, status: &str, conclusion: Option<&str>) -> String {
        let conclusion = conclusion
            .map(|value| format!(r#""{value}""#))
            .unwrap_or_else(|| "null".to_owned());
        format!(
            r#"{{
                "id": {id},
                "name": "CI",
                "event": "push",
                "status": "{status}",
                "conclusion": {conclusion},
                "head_branch": "main",
                "head_sha": "abc123",
                "html_url": "https://github.com/acme/tool/actions/runs/{id}",
                "created_at": "2026-05-06T00:00:00Z",
                "updated_at": "2026-05-06T00:01:00Z"
            }}"#
        )
    }

    fn issue_json(number: u64, state: &str) -> String {
        format!(
            r#"{{
                "number": {number},
                "title": "Fix build",
                "body": "body",
                "state": "{state}",
                "html_url": "https://github.com/acme/tool/issues/{number}",
                "user": {{"login": "alice"}},
                "labels": [{{"name": "bug"}}],
                "assignees": [{{"login": "bob"}}],
                "comments": 1,
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z",
                "closed_at": null
            }}"#
        )
    }

    fn pull_request_issue_json(number: u64) -> String {
        let mut value = serde_json::from_str::<Value>(&issue_json(number, "open"))
            .expect("issue fixture should be JSON");
        value["pull_request"] = json!({ "url": format!("https://api.github.com/pulls/{number}") });
        serde_json::to_string(&value).expect("pull request fixture should serialize")
    }

    fn issue_comment_json(id: u64) -> String {
        format!(
            r#"{{
                "id": {id},
                "body": "hello",
                "html_url": "https://github.com/acme/tool/issues/21#issuecomment-{id}",
                "user": {{"login": "alice"}},
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z"
            }}"#
        )
    }

    fn only_request(server: &MockServer) -> CapturedRequest {
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        requests[0].clone()
    }

    fn log_zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        for (path, content) in files {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .expect("zip file should start");
            writer
                .write_all(content.as_bytes())
                .expect("zip content should write");
        }
        writer.finish().expect("zip should finish").into_inner()
    }

    #[derive(Debug, Clone)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: String,
    }

    impl CapturedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .get(&name.to_ascii_lowercase())
                .map(String::as_str)
        }
    }

    struct MockResponse {
        status: u16,
        content_type: String,
        body: Vec<u8>,
    }

    impl MockResponse {
        fn json(status: u16, body: &str) -> Self {
            Self::bytes(status, "application/json", body.as_bytes().to_vec())
        }

        fn empty(status: u16) -> Self {
            Self::bytes(status, "application/json", Vec::new())
        }

        fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Self {
            Self {
                status,
                content_type: content_type.to_owned(),
                body,
            }
        }
    }

    struct MockServer {
        url: String,
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn new(responses: Vec<MockResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("mock server should bind");
            listener
                .set_nonblocking(true)
                .expect("listener should be nonblocking");
            let url = format!(
                "http://{}",
                listener.local_addr().expect("local addr should exist")
            );
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&requests);
            let handle = thread::spawn(move || {
                // Generous on purpose. This bounds a genuinely stuck test; it is
                // not a latency assertion, and five seconds is not enough when
                // the whole workspace is building and testing in parallel.
                let deadline = Instant::now() + Duration::from_secs(60);
                for response in responses {
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                handle_connection(stream, response, &captured);
                                break;
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                if Instant::now() > deadline {
                                    return;
                                }
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => return,
                        }
                    }
                }
            });

            Self {
                url,
                requests,
                handle: Some(handle),
            }
        }

        fn url(&self) -> String {
            self.url.clone()
        }

        fn requests(&self) -> Vec<CapturedRequest> {
            if let Some(handle) = &self.handle {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !handle.is_finished() {
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
            self.requests.lock().expect("requests lock").clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_connection(
        mut stream: TcpStream,
        response: MockResponse,
        requests: &Arc<Mutex<Vec<CapturedRequest>>>,
    ) {
        stream
            .set_nonblocking(false)
            .expect("accepted stream should be blocking");
        let mut reader = BufReader::new(stream.try_clone().expect("stream should clone"));
        let mut first_line = String::new();
        reader
            .read_line(&mut first_line)
            .expect("request line should read");
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_owned();
        let path = parts.next().unwrap_or("").to_owned();

        let mut headers = HashMap::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header should read");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                let key = name.trim().to_ascii_lowercase();
                let value = value.trim().to_owned();
                if key == "content-length" {
                    content_length = value.parse::<usize>().unwrap_or(0);
                }
                headers.insert(key, value);
            }
        }

        let mut body_bytes = vec![0; content_length];
        if content_length > 0 {
            reader
                .read_exact(&mut body_bytes)
                .expect("request body should read");
        }
        let body = String::from_utf8_lossy(&body_bytes).into_owned();
        requests
            .lock()
            .expect("requests lock")
            .push(CapturedRequest {
                method,
                path,
                headers,
                body,
            });

        let status_text = match response.status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            404 => "Not Found",
            _ => "OK",
        };
        let headers = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.status,
            status_text,
            response.content_type,
            response.body.len()
        );
        stream
            .write_all(headers.as_bytes())
            .expect("response headers should write");
        stream
            .write_all(&response.body)
            .expect("response body should write");
    }
}
