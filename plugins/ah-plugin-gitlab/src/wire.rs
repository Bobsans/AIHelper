//! The GitLab REST and GraphQL APIs as JSON, and the payloads this plugin
//! publishes.
//!
//! Two kinds of type live here and they are deliberately separate: a
//! `*Response` is GitLab's shape, deserialised from the wire, and a `*Output`
//! is what `ah` prints. The `Graphql*` types are the design query's envelope,
//! which is the one call that is not REST. Nothing here has behaviour.

use super::*;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) remote_url: Option<String>,
    pub(crate) host: String,
    pub(crate) api_url: String,
    pub(crate) id: Option<u64>,
    pub(crate) path_with_namespace: Option<String>,
    pub(crate) web_url: Option<String>,
    pub(crate) default_branch: Option<String>,
    pub(crate) visibility: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct GitlabProjectResponse {
    pub(crate) id: Option<u64>,
    pub(crate) path_with_namespace: Option<String>,
    pub(crate) web_url: Option<String>,
    pub(crate) default_branch: Option<String>,
    pub(crate) visibility: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct GitlabUser {
    pub(crate) id: Option<u64>,
    pub(crate) username: Option<String>,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct IssueResponse {
    pub(crate) id: u64,
    pub(crate) iid: u64,
    pub(crate) project_id: Option<u64>,
    pub(crate) title: String,
    pub(crate) description: Option<String>,
    pub(crate) state: String,
    pub(crate) web_url: Option<String>,
    pub(crate) author: Option<GitlabUser>,
    #[serde(default)]
    pub(crate) assignees: Option<Vec<GitlabUser>>,
    #[serde(default)]
    pub(crate) labels: Vec<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) closed_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssuesOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) state: String,
    pub(crate) labels: Vec<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) search: Option<String>,
    pub(crate) issue_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) issues: Vec<IssueResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) issue: IssueResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct IssueNoteResponse {
    pub(crate) id: u64,
    pub(crate) body: Option<String>,
    pub(crate) author: Option<GitlabUser>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) system: Option<bool>,
    pub(crate) web_url: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueNotesOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    pub(crate) comment_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) comments: Vec<IssueNoteResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueNoteOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) comment: IssueNoteResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct IssueDesignResponse {
    pub(crate) id: Option<String>,
    pub(crate) filename: Option<String>,
    #[serde(rename = "fullPath")]
    pub(crate) full_path: Option<String>,
    pub(crate) image: Option<String>,
    #[serde(rename = "imageV432x230")]
    pub(crate) image_v432x230: Option<String>,
    #[serde(rename = "notesCount")]
    pub(crate) notes_count: Option<u64>,
    pub(crate) event: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GraphqlEnvelope<T> {
    pub(crate) data: Option<T>,
    #[serde(default)]
    pub(crate) errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GraphqlError {
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDesignsGraphqlData {
    pub(crate) project: Option<IssueDesignsGraphqlProject>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDesignsGraphqlProject {
    pub(crate) issue: Option<IssueDesignsGraphqlIssue>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDesignsGraphqlIssue {
    #[serde(rename = "designCollection")]
    pub(crate) design_collection: Option<IssueDesignCollection>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDesignCollection {
    pub(crate) designs: Option<IssueDesignConnection>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDesignConnection {
    #[serde(default)]
    pub(crate) nodes: Vec<IssueDesignResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueFullOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(range(min = 1))]
    pub(crate) iid: u64,
    pub(crate) full: bool,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) issue: IssueResponse,
    pub(crate) comment_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) comments: Vec<IssueNoteResponse>,
    pub(crate) design_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) designs: Vec<IssueDesignResponse>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct ReleaseResponse {
    pub(crate) tag_name: String,
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) released_at: Option<String>,
    pub(crate) upcoming_release: Option<bool>,
    pub(crate) assets: Option<Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleasesOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) release_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) releases: Vec<ReleaseResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) release: ReleaseResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct PipelineResponse {
    pub(crate) id: u64,
    pub(crate) iid: Option<u64>,
    pub(crate) project_id: Option<u64>,
    pub(crate) sha: Option<String>,
    pub(crate) r#ref: Option<String>,
    pub(crate) status: String,
    pub(crate) source: Option<String>,
    pub(crate) web_url: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PipelinesOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) branch: Option<String>,
    pub(crate) pipeline_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) pipelines: Vec<PipelineResponse>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PipelineOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) pipeline: PipelineResponse,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitPipelineOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(schema_with = "ah_plugin_api::schema::external_object")]
    pub(crate) pipeline: PipelineResponse,
    pub(crate) elapsed_secs: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub(crate) struct JobResponse {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) status: String,
    pub(crate) stage: Option<String>,
    pub(crate) r#ref: Option<String>,
    pub(crate) allow_failure: Option<bool>,
    pub(crate) web_url: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) started_at: Option<String>,
    pub(crate) finished_at: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct JobsOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(range(min = 1))]
    pub(crate) pipeline_id: u64,
    pub(crate) job_count: usize,
    #[schemars(schema_with = "ah_plugin_api::schema::external_array")]
    pub(crate) jobs: Vec<JobResponse>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceLine {
    #[schemars(range(min = 1))]
    pub(crate) line: usize,
    pub(crate) text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TraceOutput {
    pub(crate) command: &'static str,
    pub(crate) project: String,
    #[schemars(range(min = 1))]
    pub(crate) job_id: u64,
    pub(crate) grep: Option<String>,
    pub(crate) match_count: usize,
    pub(crate) truncated: bool,
    pub(crate) matches: Vec<TraceLine>,
}
