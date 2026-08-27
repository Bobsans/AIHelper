//! The GitHub REST API as JSON, and the payloads this plugin publishes.
//!
//! Two kinds of type live here and they are deliberately separate: a
//! `*Response` is GitHub's shape, deserialised from the wire, and a `*Output`
//! is what `ah` prints. Nothing here has behaviour.

use super::*;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RepoOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) remote_url: Option<String>,
    pub(crate) api_url: String,
    pub(crate) html_url: Option<String>,
    pub(crate) default_branch: Option<String>,
    pub(crate) private: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct GithubRepoResponse {
    pub(crate) full_name: Option<String>,
    pub(crate) html_url: Option<String>,
    pub(crate) default_branch: Option<String>,
    pub(crate) private: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct GithubUser {
    pub(crate) login: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct GithubLabel {
    pub(crate) name: String,
}

/// GitHub owns the shape of a linked pull request, and it is absent for a plain
/// issue, so the published schema stays open and nullable.
pub(crate) fn nullable_external_object(
    generator: &mut schemars::SchemaGenerator,
) -> schemars::Schema {
    let object = ah_plugin_api::schema::external_object(generator);
    schemars::json_schema!({"oneOf": [object, {"type": "null"}]})
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct IssueResponse {
    #[schemars(range(min = 1))]
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) state: String,
    pub(crate) html_url: Option<String>,
    pub(crate) user: Option<GithubUser>,
    #[serde(default)]
    pub(crate) labels: Vec<GithubLabel>,
    #[serde(default)]
    pub(crate) assignees: Vec<GithubUser>,
    pub(crate) comments: Option<u64>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    // GitHub owns this sub-object and only its presence matters here.
    #[serde(default)]
    #[schemars(schema_with = "nullable_external_object")]
    pub(crate) pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueSearchResponse {
    pub(crate) items: Vec<IssueResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssuesOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) state: String,
    pub(crate) labels: Vec<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) search: Option<String>,
    pub(crate) issue_count: usize,
    pub(crate) issues: Vec<IssueResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) issue: IssueResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct IssueCommentResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) body: Option<String>,
    pub(crate) html_url: Option<String>,
    pub(crate) user: Option<GithubUser>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueCommentsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    #[schemars(range(min = 1))]
    pub(crate) number: u64,
    pub(crate) comment_count: usize,
    pub(crate) comments: Vec<IssueCommentResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueCommentOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    #[schemars(range(min = 1))]
    pub(crate) number: u64,
    pub(crate) comment: IssueCommentResponse,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ReleaseResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) tag_name: String,
    pub(crate) name: Option<String>,
    pub(crate) draft: bool,
    pub(crate) prerelease: bool,
    pub(crate) html_url: Option<String>,
    pub(crate) published_at: Option<String>,
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ReleaseAsset {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) size: u64,
    pub(crate) browser_download_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) release: ReleaseResponse,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseAssetsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) tag: String,
    pub(crate) asset_count: usize,
    pub(crate) assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct WorkflowListResponse {
    pub(crate) workflows: Vec<WorkflowResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct WorkflowResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) state: String,
    pub(crate) html_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) workflow_count: usize,
    pub(crate) workflows: Vec<WorkflowResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkflowDispatchOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) workflow: String,
    pub(crate) r#ref: String,
    pub(crate) input_count: usize,
    pub(crate) dispatched: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct RunsListResponse {
    pub(crate) workflow_runs: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Deserialize, Serialize, Clone, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct WorkflowRunResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) name: Option<String>,
    pub(crate) event: String,
    pub(crate) status: String,
    pub(crate) conclusion: Option<String>,
    pub(crate) head_branch: Option<String>,
    pub(crate) head_sha: String,
    pub(crate) html_url: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) workflow: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) run_count: usize,
    pub(crate) runs: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) run: WorkflowRunResponse,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitRunOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    pub(crate) run: WorkflowRunResponse,
    pub(crate) elapsed_secs: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct JobsListResponse {
    pub(crate) jobs: Vec<JobResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct JobResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) conclusion: Option<String>,
    pub(crate) html_url: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    #[schemars(range(min = 1))]
    pub(crate) run_id: u64,
    pub(crate) job_count: usize,
    pub(crate) jobs: Vec<JobResponse>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogLine {
    pub(crate) file: String,
    #[schemars(range(min = 1))]
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    #[schemars(range(min = 1))]
    pub(crate) run_id: u64,
    pub(crate) grep: Option<String>,
    pub(crate) match_count: usize,
    pub(crate) truncated: bool,
    pub(crate) matches: Vec<LogLine>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactsOutput {
    pub(crate) command: &'static str,
    pub(crate) repository: String,
    #[schemars(range(min = 1))]
    pub(crate) run_id: u64,
    pub(crate) artifact_count: usize,
    pub(crate) artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct ArtifactsListResponse {
    pub(crate) artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub(crate) struct ArtifactResponse {
    #[schemars(range(min = 1))]
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) size_in_bytes: u64,
    pub(crate) expired: bool,
    pub(crate) archive_download_url: Option<String>,
}
