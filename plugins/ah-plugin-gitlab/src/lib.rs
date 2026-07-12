#![allow(clippy::result_large_err)]

use std::{
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(test)]
use ah_plugin_api::InvocationRequest;
use ah_plugin_api::{
    GlobalOptionsWire, InvocationResponse, ManualCommand, ManualExample, PluginManual,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::{Method, blocking::Client};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

const DOMAIN: &str = "gitlab";
const PLUGIN_NAME: &str = "external-gitlab";
const DESCRIPTION: &str = "GitLab Releases and Pipelines plugin (dynamic)";
const DEFAULT_HOST: &str = "https://gitlab.com";
const DEFAULT_REMOTE: &str = "origin";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 15;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 1800;
const DEFAULT_MAX_TRACE_BODY_BYTES: usize = 8 * 1024 * 1024;
const ISSUE_DESIGNS_QUERY: &str = r#"
query IssueDesigns($fullPath: ID!, $iid: String!, $first: Int!) {
  project(fullPath: $fullPath) {
    issue(iid: $iid) {
      designCollection {
        designs(first: $first) {
          nodes {
            id
            filename
            fullPath
            image
            imageV432x230
            notesCount
            event
          }
        }
      }
    }
  }
}
"#;

static PLUGIN_NAME_C: &[u8] = b"external-gitlab\0";
static DOMAIN_C: &[u8] = b"gitlab\0";
static DESCRIPTION_C: &[u8] = b"GitLab Releases and Pipelines plugin (dynamic)\0";

ah_plugin_api::define_plugin_entrypoint_v1!(
    plugin_name_c: PLUGIN_NAME_C,
    domain_c: DOMAIN_C,
    description_c: DESCRIPTION_C,
    domain: DOMAIN,
    parse_fn: parse_args,
    execute_fn: execute,
    manual_fn: plugin_manual,
);

#[derive(Debug, Parser)]
#[command(name = "gitlab", about = "GitLab release and pipeline helpers")]
struct GitlabCli {
    #[command(flatten)]
    connection: GitlabConnectionArgs,
    #[command(subcommand)]
    command: GitlabCommand,
}

#[derive(Debug, Args, Clone)]
struct GitlabConnectionArgs {
    #[arg(long, global = true, value_name = "PATH_OR_ID")]
    project: Option<String>,
    #[arg(long, global = true, default_value = DEFAULT_REMOTE, value_name = "NAME")]
    remote: String,
    #[arg(long, global = true, default_value = DEFAULT_HOST, value_name = "URL")]
    host: String,
    #[arg(long, global = true, value_name = "URL")]
    api_url: Option<String>,
    #[arg(long, global = true, value_name = "URL")]
    graphql_url: Option<String>,
    #[arg(long, global = true, value_name = "TOKEN")]
    token: Option<String>,
    #[arg(long, global = true)]
    use_git_credential: bool,
    #[arg(long, global = true, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    timeout_secs: u64,
}

#[derive(Debug, Subcommand)]
enum GitlabCommand {
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

#[derive(Debug, Args)]
struct IssuesArgs {
    #[arg(long, default_value = "opened", value_parser = ["opened", "closed", "all"])]
    state: String,
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    #[arg(long)]
    assignee: Option<String>,
    #[arg(long)]
    author: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    search: Option<String>,
}

#[derive(Debug, Args)]
struct IssueArgs {
    #[command(subcommand)]
    command: IssueCommand,
}

#[derive(Debug, Subcommand)]
enum IssueCommand {
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

#[derive(Debug, Args)]
struct IssueIidArgs {
    iid: u64,
}

#[derive(Debug, Args)]
struct IssueViewArgs {
    iid: u64,
    #[arg(long)]
    full: bool,
}

#[derive(Debug, Args)]
struct CreateIssueArgs {
    #[arg(long)]
    title: String,
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    #[arg(long, value_name = "PATH")]
    description_file: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    #[arg(long = "assignee-id", value_name = "ID")]
    assignee_ids: Vec<u64>,
}

#[derive(Debug, Args)]
struct UpdateIssueArgs {
    iid: u64,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    #[arg(long, value_name = "PATH")]
    description_file: Option<String>,
    #[arg(long, value_parser = ["opened", "closed"])]
    state: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    #[arg(long = "assignee-id", value_name = "ID")]
    assignee_ids: Vec<u64>,
}

#[derive(Debug, Args)]
struct CloseIssueArgs {
    iid: u64,
    #[arg(long, value_name = "TEXT")]
    comment: Option<String>,
    #[arg(long, value_name = "PATH")]
    comment_file: Option<String>,
}

#[derive(Debug, Args)]
struct CommentIssueArgs {
    iid: u64,
    #[arg(long, value_name = "TEXT")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
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
    #[command(about = "Create a GitLab release for an existing or new tag")]
    Create(CreateReleaseArgs),
}

#[derive(Debug, Args)]
struct TagArgs {
    tag: String,
}

#[derive(Debug, Args)]
struct CreateReleaseArgs {
    tag: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    #[arg(long, value_name = "PATH")]
    description_file: Option<String>,
    #[arg(long)]
    r#ref: Option<String>,
}

#[derive(Debug, Args)]
struct PipelinesArgs {
    #[arg(long, value_name = "BRANCH")]
    branch: Option<String>,
}

#[derive(Debug, Args)]
struct PipelineArgs {
    #[command(subcommand)]
    command: PipelineCommand,
}

#[derive(Debug, Subcommand)]
enum PipelineCommand {
    #[command(about = "Get pipeline metadata")]
    Get(PipelineIdArgs),
    #[command(about = "Wait for pipeline completion")]
    Wait(WaitPipelineArgs),
    #[command(about = "List pipeline jobs")]
    Jobs(PipelineIdArgs),
}

#[derive(Debug, Args)]
struct PipelineIdArgs {
    pipeline_id: u64,
}

#[derive(Debug, Args)]
struct WaitPipelineArgs {
    pipeline_id: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_INTERVAL_SECS, value_name = "SECONDS")]
    interval_secs: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT_SECS, value_name = "SECONDS")]
    timeout_secs: u64,
    #[arg(long)]
    fail_on_failure: bool,
}

#[derive(Debug, Args)]
struct JobArgs {
    #[command(subcommand)]
    command: JobCommand,
}

#[derive(Debug, Subcommand)]
enum JobCommand {
    #[command(about = "Read job trace")]
    Trace(JobTraceArgs),
    #[command(about = "Extract warning-like lines from job trace")]
    Warnings(JobTraceReadArgs),
}

#[derive(Debug, Args)]
struct JobTraceArgs {
    job_id: u64,
    #[arg(long)]
    grep: Option<String>,
    #[command(flatten)]
    limits: JobTraceLimitArgs,
}

#[derive(Debug, Args)]
struct JobTraceReadArgs {
    job_id: u64,
    #[command(flatten)]
    limits: JobTraceLimitArgs,
}

#[derive(Debug, Args)]
struct JobTraceLimitArgs {
    #[arg(
        long,
        default_value_t = DEFAULT_MAX_TRACE_BODY_BYTES,
        value_name = "BYTES"
    )]
    max_body_bytes: usize,
}

#[derive(Debug, Clone)]
struct ProjectRef {
    value: String,
}

impl ProjectRef {
    fn encoded(&self) -> String {
        urlencoding::encode(&self.value).into_owned()
    }
}

#[derive(Debug)]
struct GitlabContext {
    client: Client,
    host: String,
    api_url: String,
    graphql_url: String,
    token: Option<String>,
    project: ProjectRef,
    remote_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProjectOutput {
    command: &'static str,
    project: String,
    remote_url: Option<String>,
    host: String,
    api_url: String,
    id: Option<u64>,
    path_with_namespace: Option<String>,
    web_url: Option<String>,
    default_branch: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GitlabProjectResponse {
    id: Option<u64>,
    path_with_namespace: Option<String>,
    web_url: Option<String>,
    default_branch: Option<String>,
    visibility: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GitlabUser {
    id: Option<u64>,
    username: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IssueResponse {
    id: u64,
    iid: u64,
    project_id: Option<u64>,
    title: String,
    description: Option<String>,
    state: String,
    web_url: Option<String>,
    author: Option<GitlabUser>,
    #[serde(default)]
    assignees: Option<Vec<GitlabUser>>,
    #[serde(default)]
    labels: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct IssuesOutput {
    command: &'static str,
    project: String,
    state: String,
    labels: Vec<String>,
    assignee: Option<String>,
    author: Option<String>,
    since: Option<String>,
    search: Option<String>,
    issue_count: usize,
    issues: Vec<IssueResponse>,
}

#[derive(Debug, Serialize)]
struct IssueOutput {
    command: &'static str,
    project: String,
    issue: IssueResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IssueNoteResponse {
    id: u64,
    body: Option<String>,
    author: Option<GitlabUser>,
    created_at: Option<String>,
    updated_at: Option<String>,
    system: Option<bool>,
    web_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct IssueNotesOutput {
    command: &'static str,
    project: String,
    iid: u64,
    comment_count: usize,
    comments: Vec<IssueNoteResponse>,
}

#[derive(Debug, Serialize)]
struct IssueNoteOutput {
    command: &'static str,
    project: String,
    iid: u64,
    comment: IssueNoteResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IssueDesignResponse {
    id: Option<String>,
    filename: Option<String>,
    #[serde(rename = "fullPath")]
    full_path: Option<String>,
    image: Option<String>,
    #[serde(rename = "imageV432x230")]
    image_v432x230: Option<String>,
    #[serde(rename = "notesCount")]
    notes_count: Option<u64>,
    event: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GraphqlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct IssueDesignsGraphqlData {
    project: Option<IssueDesignsGraphqlProject>,
}

#[derive(Debug, Deserialize)]
struct IssueDesignsGraphqlProject {
    issue: Option<IssueDesignsGraphqlIssue>,
}

#[derive(Debug, Deserialize)]
struct IssueDesignsGraphqlIssue {
    #[serde(rename = "designCollection")]
    design_collection: Option<IssueDesignCollection>,
}

#[derive(Debug, Deserialize)]
struct IssueDesignCollection {
    designs: Option<IssueDesignConnection>,
}

#[derive(Debug, Deserialize)]
struct IssueDesignConnection {
    #[serde(default)]
    nodes: Vec<IssueDesignResponse>,
}

#[derive(Debug, Serialize)]
struct IssueFullOutput {
    command: &'static str,
    project: String,
    iid: u64,
    full: bool,
    issue: IssueResponse,
    comment_count: usize,
    comments: Vec<IssueNoteResponse>,
    design_count: usize,
    designs: Vec<IssueDesignResponse>,
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ReleaseResponse {
    tag_name: String,
    name: Option<String>,
    description: Option<String>,
    created_at: Option<String>,
    released_at: Option<String>,
    upcoming_release: Option<bool>,
    assets: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ReleasesOutput {
    command: &'static str,
    project: String,
    release_count: usize,
    releases: Vec<ReleaseResponse>,
}

#[derive(Debug, Serialize)]
struct ReleaseOutput {
    command: &'static str,
    project: String,
    release: ReleaseResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct PipelineResponse {
    id: u64,
    iid: Option<u64>,
    project_id: Option<u64>,
    sha: Option<String>,
    r#ref: Option<String>,
    status: String,
    source: Option<String>,
    web_url: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct PipelinesOutput {
    command: &'static str,
    project: String,
    branch: Option<String>,
    pipeline_count: usize,
    pipelines: Vec<PipelineResponse>,
}

#[derive(Debug, Serialize)]
struct PipelineOutput {
    command: &'static str,
    project: String,
    pipeline: PipelineResponse,
}

#[derive(Debug, Serialize)]
struct WaitPipelineOutput {
    command: &'static str,
    project: String,
    pipeline: PipelineResponse,
    elapsed_secs: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct JobResponse {
    id: u64,
    name: String,
    status: String,
    stage: Option<String>,
    r#ref: Option<String>,
    allow_failure: Option<bool>,
    web_url: Option<String>,
    created_at: Option<String>,
    started_at: Option<String>,
    finished_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct JobsOutput {
    command: &'static str,
    project: String,
    pipeline_id: u64,
    job_count: usize,
    jobs: Vec<JobResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct TraceLine {
    line: usize,
    text: String,
}

#[derive(Debug, Serialize)]
struct TraceOutput {
    command: &'static str,
    project: String,
    job_id: u64,
    grep: Option<String>,
    match_count: usize,
    truncated: bool,
    matches: Vec<TraceLine>,
}

fn parse_args(argv: &[String]) -> Result<GitlabCli, InvocationResponse> {
    let mut args = Vec::with_capacity(argv.len() + 1);
    args.push(DOMAIN.to_owned());
    args.extend(argv.iter().cloned());

    match GitlabCli::try_parse_from(args) {
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

fn execute(cli: GitlabCli, globals: &GlobalOptionsWire) -> InvocationResponse {
    let context = match gitlab_context(&cli.connection) {
        Ok(value) => value,
        Err(error) => return error,
    };

    match cli.command {
        GitlabCommand::Project => execute_project(&context, globals),
        GitlabCommand::Issues(args) => execute_issues(args, &context, globals),
        GitlabCommand::Issue(args) => execute_issue(args, &context, globals),
        GitlabCommand::Releases => execute_releases(&context, globals),
        GitlabCommand::Release(args) => execute_release(args, &context, globals),
        GitlabCommand::Pipelines(args) => execute_pipelines(args, &context, globals),
        GitlabCommand::Pipeline(args) => execute_pipeline(args, &context, globals),
        GitlabCommand::Job(args) => execute_job(args, &context, globals),
    }
}

fn execute_project(context: &GitlabContext, globals: &GlobalOptionsWire) -> InvocationResponse {
    let path = format!("/projects/{}", context.project.encoded());
    let project_response = gitlab_json::<GitlabProjectResponse>(context, Method::GET, &path, None);
    let (id, path_with_namespace, web_url, default_branch, visibility) = match project_response {
        Ok(value) => (
            value.id,
            value.path_with_namespace,
            value.web_url,
            value.default_branch,
            value.visibility,
        ),
        Err(_) => (None, None, None, None, None),
    };

    let output = ProjectOutput {
        command: "gitlab.project",
        project: context.project.value.clone(),
        remote_url: context.remote_url.clone(),
        host: context.host.clone(),
        api_url: context.api_url.clone(),
        id,
        path_with_namespace,
        web_url,
        default_branch,
        visibility,
    };

    render_success(globals, &output, format!("{}\n", output.project))
}

fn execute_issues(
    args: IssuesArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = gitlab_issues_list_path(context, &args, per_page);
    let issues = match gitlab_json::<Vec<IssueResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssuesOutput {
            command: "gitlab.issues",
            project: context.project.value.clone(),
            state: args.state,
            labels: args.labels,
            assignee: args.assignee,
            author: args.author,
            since: args.since,
            search: args.search,
            issue_count: issues.len(),
            issues: issues.clone(),
        },
        render_issues_text(&issues),
    )
}

fn execute_issue(
    args: IssueArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        IssueCommand::View(args) => {
            let issue = match get_issue(context, args.iid) {
                Ok(value) => value,
                Err(error) => return error,
            };
            if args.full {
                return issue_view_full(context, globals, args, issue);
            }
            render_success(
                globals,
                &IssueOutput {
                    command: "gitlab.issue.view",
                    project: context.project.value.clone(),
                    issue: issue.clone(),
                },
                render_issues_text(std::slice::from_ref(&issue)),
            )
        }
        IssueCommand::Create(args) => create_issue(context, args, globals),
        IssueCommand::Update(args) => update_issue(context, args, globals),
        IssueCommand::Close(args) => close_issue(context, args, globals),
        IssueCommand::Comment(args) => comment_issue(context, args, globals),
        IssueCommand::Comments(args) => issue_comments(context, args.iid, globals),
    }
}

fn issue_view_full(
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
    args: IssueViewArgs,
    issue: IssueResponse,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(100).clamp(1, 100);
    let comments = match get_issue_comments(context, args.iid, per_page) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let (designs, warnings) = match get_issue_designs_best_effort(context, args.iid, per_page) {
        Ok(value) => (value, Vec::new()),
        Err(warning) => (Vec::new(), vec![warning]),
    };
    render_success(
        globals,
        &IssueFullOutput {
            command: "gitlab.issue.view",
            project: context.project.value.clone(),
            iid: args.iid,
            full: true,
            issue: issue.clone(),
            comment_count: comments.len(),
            comments: comments.clone(),
            design_count: designs.len(),
            designs: designs.clone(),
            warnings: warnings.clone(),
        },
        render_issue_full_text(&issue, &comments, &designs, &warnings),
    )
}

fn create_issue(
    context: &GitlabContext,
    args: CreateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description =
        match resolve_optional_text(args.description, args.description_file, "description") {
            Ok(value) => value,
            Err(error) => return error,
        };
    let mut body = serde_json::Map::new();
    body.insert("title".to_owned(), Value::String(args.title));
    if let Some(description) = description {
        body.insert("description".to_owned(), Value::String(description));
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), Value::String(args.labels.join(",")));
    }
    if !args.assignee_ids.is_empty() {
        body.insert("assignee_ids".to_owned(), json!(args.assignee_ids));
    }
    let path = format!("/projects/{}/issues", context.project.encoded());
    let issue =
        match gitlab_json::<IssueResponse>(context, Method::POST, &path, Some(Value::Object(body)))
        {
            Ok(value) => value,
            Err(error) => return error,
        };
    render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.create",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
    )
}

fn update_issue(
    context: &GitlabContext,
    args: UpdateIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description =
        match resolve_optional_text(args.description, args.description_file, "description") {
            Ok(value) => value,
            Err(error) => return error,
        };
    let mut body = serde_json::Map::new();
    if let Some(title) = args.title {
        body.insert("title".to_owned(), Value::String(title));
    }
    if let Some(description) = description {
        body.insert("description".to_owned(), Value::String(description));
    }
    if let Some(state) = args.state {
        body.insert(
            "state_event".to_owned(),
            Value::String(gitlab_state_event(&state).to_owned()),
        );
    }
    if !args.labels.is_empty() {
        body.insert("labels".to_owned(), Value::String(args.labels.join(",")));
    }
    if !args.assignee_ids.is_empty() {
        body.insert("assignee_ids".to_owned(), json!(args.assignee_ids));
    }
    if body.is_empty() {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            "issue update requires at least one field",
        );
    }
    let path = format!(
        "/projects/{}/issues/{}",
        context.project.encoded(),
        args.iid
    );
    let issue = match gitlab_json::<IssueResponse>(
        context,
        Method::PUT,
        &path,
        Some(Value::Object(body)),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.update",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
    )
}

fn close_issue(
    context: &GitlabContext,
    args: CloseIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let comment = match resolve_optional_text(args.comment, args.comment_file, "comment") {
        Ok(value) => value,
        Err(error) => return error,
    };
    if let Some(comment) = comment
        && let Err(error) = create_issue_note(context, args.iid, comment)
    {
        return error;
    }
    let path = format!(
        "/projects/{}/issues/{}",
        context.project.encoded(),
        args.iid
    );
    let issue = match gitlab_json::<IssueResponse>(
        context,
        Method::PUT,
        &path,
        Some(json!({ "state_event": "close" })),
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssueOutput {
            command: "gitlab.issue.close",
            project: context.project.value.clone(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
    )
}

fn comment_issue(
    context: &GitlabContext,
    args: CommentIssueArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let body = match resolve_required_text(args.body, args.body_file, "body") {
        Ok(value) => value,
        Err(error) => return error,
    };
    let comment = match create_issue_note(context, args.iid, body) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssueNoteOutput {
            command: "gitlab.issue.comment",
            project: context.project.value.clone(),
            iid: args.iid,
            comment: comment.clone(),
        },
        render_comments_text(std::slice::from_ref(&comment)),
    )
}

fn issue_comments(
    context: &GitlabContext,
    iid: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let comments = match get_issue_comments(context, iid, per_page) {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssueNotesOutput {
            command: "gitlab.issue.comments",
            project: context.project.value.clone(),
            iid,
            comment_count: comments.len(),
            comments: comments.clone(),
        },
        render_comments_text(&comments),
    )
}

fn execute_releases(context: &GitlabContext, globals: &GlobalOptionsWire) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).clamp(1, 100);
    let path = format!(
        "/projects/{}/releases?per_page={per_page}",
        context.project.encoded()
    );
    let releases = match gitlab_json::<Vec<ReleaseResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_releases_text(&releases);
    render_success(
        globals,
        &ReleasesOutput {
            command: "gitlab.releases",
            project: context.project.value.clone(),
            release_count: releases.len(),
            releases,
        },
        text,
    )
}

fn execute_release(
    args: ReleaseArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        ReleaseCommand::Get(args) => {
            let release = match get_release(context, &args.tag) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_release_text(&release);
            render_success(
                globals,
                &ReleaseOutput {
                    command: "gitlab.release.get",
                    project: context.project.value.clone(),
                    release,
                },
                text,
            )
        }
        ReleaseCommand::Create(args) => create_release(context, args, globals),
    }
}

fn execute_pipelines(
    args: PipelinesArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(10).clamp(1, 100);
    let ref_query = args
        .branch
        .as_ref()
        .map(|branch| format!("&ref={}", urlencoding::encode(branch)))
        .unwrap_or_default();
    let path = format!(
        "/projects/{}/pipelines?per_page={per_page}{ref_query}",
        context.project.encoded()
    );
    let pipelines = match gitlab_json::<Vec<PipelineResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_pipelines_text(&pipelines);
    render_success(
        globals,
        &PipelinesOutput {
            command: "gitlab.pipelines",
            project: context.project.value.clone(),
            branch: args.branch,
            pipeline_count: pipelines.len(),
            pipelines,
        },
        text,
    )
}

fn execute_pipeline(
    args: PipelineArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        PipelineCommand::Get(args) => {
            let pipeline = match get_pipeline(context, args.pipeline_id) {
                Ok(value) => value,
                Err(error) => return error,
            };
            let text = render_pipelines_text(std::slice::from_ref(&pipeline));
            render_success(
                globals,
                &PipelineOutput {
                    command: "gitlab.pipeline.get",
                    project: context.project.value.clone(),
                    pipeline,
                },
                text,
            )
        }
        PipelineCommand::Wait(args) => wait_pipeline(context, args, globals),
        PipelineCommand::Jobs(args) => pipeline_jobs(context, args.pipeline_id, globals),
    }
}

fn execute_job(
    args: JobArgs,
    context: &GitlabContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        JobCommand::Trace(args) => {
            job_trace(context, args.job_id, args.grep, args.limits, globals, false)
        }
        JobCommand::Warnings(args) => {
            job_trace(context, args.job_id, None, args.limits, globals, true)
        }
    }
}

fn create_release(
    context: &GitlabContext,
    args: CreateReleaseArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let description = match (args.description, args.description_file) {
        (Some(description), None) => Some(description),
        (None, Some(path)) => match fs::read_to_string(&path) {
            Ok(value) => Some(value),
            Err(error) => {
                return InvocationResponse::error(
                    "FILE_READ_FAILED",
                    format!("failed to read description file '{path}': {error}"),
                );
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                "use either --description or --description-file, not both",
            );
        }
    };
    let body = json!({
        "tag_name": args.tag,
        "name": args.name,
        "description": description,
        "ref": args.r#ref,
    });
    let path = format!("/projects/{}/releases", context.project.encoded());
    let release = match gitlab_json::<ReleaseResponse>(context, Method::POST, &path, Some(body)) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_release_text(&release);
    render_success(
        globals,
        &ReleaseOutput {
            command: "gitlab.release.create",
            project: context.project.value.clone(),
            release,
        },
        text,
    )
}

fn wait_pipeline(
    context: &GitlabContext,
    args: WaitPipelineArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let start = Instant::now();
    let timeout = Duration::from_secs(args.timeout_secs.max(1));
    let interval = Duration::from_secs(args.interval_secs.max(1));

    loop {
        let pipeline = match get_pipeline(context, args.pipeline_id) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if is_pipeline_terminal(&pipeline.status) {
            if args.fail_on_failure && pipeline.status != "success" {
                return InvocationResponse::error(
                    "GITLAB_PIPELINE_FAILED",
                    format!(
                        "pipeline {} completed with status {}",
                        pipeline.id, pipeline.status
                    ),
                );
            }
            let elapsed_secs = start.elapsed().as_secs();
            let text = render_pipelines_text(std::slice::from_ref(&pipeline));
            return render_success(
                globals,
                &WaitPipelineOutput {
                    command: "gitlab.pipeline.wait",
                    project: context.project.value.clone(),
                    pipeline,
                    elapsed_secs,
                },
                text,
            );
        }

        let elapsed = start.elapsed();
        if elapsed >= timeout {
            return pipeline_timeout_response(&args);
        }
        thread::sleep(interval.min(timeout - elapsed));
        if start.elapsed() >= timeout {
            return pipeline_timeout_response(&args);
        }
    }
}

fn pipeline_timeout_response(args: &WaitPipelineArgs) -> InvocationResponse {
    InvocationResponse::error(
        "GITLAB_PIPELINE_TIMEOUT",
        format!(
            "pipeline {} did not complete within {} seconds",
            args.pipeline_id, args.timeout_secs
        ),
    )
}

fn pipeline_jobs(
    context: &GitlabContext,
    pipeline_id: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let path = format!(
        "/projects/{}/pipelines/{pipeline_id}/jobs?per_page=100",
        context.project.encoded()
    );
    let jobs = match gitlab_json::<Vec<JobResponse>>(context, Method::GET, &path, None) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_jobs_text(&jobs);
    render_success(
        globals,
        &JobsOutput {
            command: "gitlab.pipeline.jobs",
            project: context.project.value.clone(),
            pipeline_id,
            job_count: jobs.len(),
            jobs,
        },
        text,
    )
}

fn job_trace(
    context: &GitlabContext,
    job_id: u64,
    grep: Option<String>,
    limits: JobTraceLimitArgs,
    globals: &GlobalOptionsWire,
    warnings_only: bool,
) -> InvocationResponse {
    if limits.max_body_bytes == 0 {
        return InvocationResponse::error("INVALID_ARGUMENT", "--max-body-bytes must be >= 1");
    }
    let (matches, truncated) = match collect_job_trace(
        context,
        job_id,
        grep.as_deref(),
        warnings_only,
        globals.limit,
        limits.max_body_bytes,
    ) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = matches
        .iter()
        .map(|line| format!("{}: {}", line.line, line.text))
        .collect::<Vec<_>>()
        .join("\n")
        + if matches.is_empty() { "" } else { "\n" };
    render_success(
        globals,
        &TraceOutput {
            command: if warnings_only {
                "gitlab.job.warnings"
            } else {
                "gitlab.job.trace"
            },
            project: context.project.value.clone(),
            job_id,
            grep,
            match_count: matches.len(),
            truncated,
            matches,
        },
        text,
    )
}

fn get_release(context: &GitlabContext, tag: &str) -> Result<ReleaseResponse, InvocationResponse> {
    let path = format!(
        "/projects/{}/releases/{}",
        context.project.encoded(),
        urlencoding::encode(tag)
    );
    gitlab_json::<ReleaseResponse>(context, Method::GET, &path, None)
}

fn get_pipeline(
    context: &GitlabContext,
    pipeline_id: u64,
) -> Result<PipelineResponse, InvocationResponse> {
    let path = format!(
        "/projects/{}/pipelines/{pipeline_id}",
        context.project.encoded()
    );
    gitlab_json::<PipelineResponse>(context, Method::GET, &path, None)
}

fn gitlab_context(args: &GitlabConnectionArgs) -> Result<GitlabContext, InvocationResponse> {
    let host = normalize_host(&args.host)?;
    let api_url = normalize_api_url(args.api_url.as_deref(), &host)?;
    let graphql_url = normalize_graphql_url(args.graphql_url.as_deref(), &api_url, &host)?;
    let (project, remote_url) = resolve_project(args, &host)?;
    let token = resolve_token(args, &host);
    let client = Client::builder()
        .timeout(Duration::from_secs(args.timeout_secs.max(1)))
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "GITLAB_HTTP_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;

    Ok(GitlabContext {
        client,
        host,
        api_url,
        graphql_url,
        token,
        project,
        remote_url,
    })
}

fn resolve_project(
    args: &GitlabConnectionArgs,
    host: &str,
) -> Result<(ProjectRef, Option<String>), InvocationResponse> {
    if let Some(project) = &args.project {
        return parse_project_ref(project)
            .map(|project| (project, None))
            .ok_or_else(|| invalid_project(project));
    }

    let remote_url = read_git_remote_url(&args.remote)?;
    parse_gitlab_remote_url(&remote_url, host)
        .map(|project| (project, Some(remote_url.clone())))
        .ok_or_else(|| {
            InvocationResponse::error(
                "GITLAB_PROJECT_UNDETECTED",
                format!(
                    "could not detect GitLab project from remote '{}' for host '{}': {}",
                    args.remote, host, remote_url
                ),
            )
        })
}

fn read_git_remote_url(remote: &str) -> Result<String, InvocationResponse> {
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .output()
        .map_err(|error| {
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

fn parse_project_ref(value: &str) -> Option<ProjectRef> {
    let normalized = value.trim().trim_matches('/').trim_end_matches(".git");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().all(|ch| ch.is_ascii_digit()) {
        return Some(ProjectRef {
            value: normalized.to_owned(),
        });
    }
    if normalized.split('/').count() < 2 || normalized.split('/').any(str::is_empty) {
        return None;
    }
    Some(ProjectRef {
        value: normalized.to_owned(),
    })
}

fn parse_gitlab_remote_url(remote: &str, host: &str) -> Option<ProjectRef> {
    let host_authority = host_authority(host)?;
    let trimmed = remote.trim();

    if let Some(rest) = trimmed.strip_prefix("git@") {
        let (remote_host, path) = rest.split_once(':')?;
        if remote_host.eq_ignore_ascii_case(&host_authority) {
            return parse_project_ref(path);
        }
    }

    for prefix in ["https://", "http://"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let (remote_host, path) = rest.split_once('/')?;
            if remote_host.eq_ignore_ascii_case(&host_authority) {
                return parse_project_ref(path);
            }
        }
    }

    if let Some(rest) = trimmed.strip_prefix("ssh://git@") {
        let (_, path) = split_authority_and_path(rest)?;
        if authority_matches_host(rest, &host_authority) {
            return parse_project_ref(path);
        }
    }

    None
}

fn split_authority_and_path(value: &str) -> Option<(&str, &str)> {
    let slash = value.find('/')?;
    Some((&value[..slash], &value[slash + 1..]))
}

fn authority_matches_host(value: &str, expected_host: &str) -> bool {
    let Some((authority, _)) = split_authority_and_path(value) else {
        return false;
    };
    let host_without_port = authority.split(':').next().unwrap_or(authority);
    host_without_port.eq_ignore_ascii_case(expected_host)
}

fn invalid_project(value: &str) -> InvocationResponse {
    InvocationResponse::error(
        "INVALID_ARGUMENT",
        format!("--project must use group/project path or numeric id, got '{value}'"),
    )
}

fn normalize_host(value: &str) -> Result<String, InvocationResponse> {
    let normalized = value.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--host must not be empty",
        ));
    }
    if !normalized.starts_with("https://") && !normalized.starts_with("http://") {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--host must start with http:// or https://",
        ));
    }
    Ok(normalized)
}

fn normalize_api_url(value: Option<&str>, host: &str) -> Result<String, InvocationResponse> {
    let raw = value
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{host}/api/v4"));
    let normalized = raw.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "--api-url must not be empty",
        ));
    }
    Ok(normalized)
}

fn normalize_graphql_url(
    explicit_graphql_url: Option<&str>,
    api_url: &str,
    host: &str,
) -> Result<String, InvocationResponse> {
    let graphql_url = if let Some(value) = explicit_graphql_url {
        value.to_owned()
    } else if let Some(prefix) = api_url.strip_suffix("/api/v4") {
        format!("{prefix}/api/graphql")
    } else {
        format!("{host}/api/graphql")
    };
    let normalized = graphql_url.trim().trim_end_matches('/').to_owned();
    if normalized.is_empty() {
        return Err(InvocationResponse::error(
            "INVALID_ARGUMENT",
            "GraphQL URL must not be empty",
        ));
    }
    Ok(normalized)
}

fn host_authority(host: &str) -> Option<String> {
    let without_scheme = host
        .strip_prefix("https://")
        .or_else(|| host.strip_prefix("http://"))?;
    Some(without_scheme.split('/').next()?.to_owned())
}

fn resolve_token(args: &GitlabConnectionArgs, host: &str) -> Option<String> {
    args.token
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env_token("GITLAB_TOKEN"))
        .or_else(|| env_token("GL_TOKEN"))
        .or_else(|| {
            if args.use_git_credential {
                git_credential_token(host)
            } else {
                None
            }
        })
}

fn env_token(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn git_credential_token(host: &str) -> Option<String> {
    let host = host_authority(host)?;
    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("protocol=https\nhost={host}\n\n").as_bytes())
            .ok()?;
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("password=") {
            let token = value.trim().to_owned();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

fn gitlab_json<T>(
    context: &GitlabContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    let response = gitlab_response(context, method, path, body)?;
    response.json::<T>().map_err(|error| {
        InvocationResponse::error(
            "GITLAB_RESPONSE_INVALID",
            format!("failed to decode GitLab response for '{path}': {error}"),
        )
    })
}

fn gitlab_graphql<T>(context: &GitlabContext, body: Value) -> Result<GraphqlEnvelope<T>, String>
where
    T: DeserializeOwned,
{
    let mut request = context
        .client
        .request(Method::POST, &context.graphql_url)
        .header("Accept", "application/json")
        .header("User-Agent", "AIHelper-gitlab-plugin");
    if let Some(token) = &context.token {
        request = request.header("PRIVATE-TOKEN", token);
    }
    let response = request
        .json(&body)
        .send()
        .map_err(|error| format!("request to '{}' failed: {error}", context.graphql_url))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "<failed to read response body>".to_owned());
        return Err(format!(
            "GitLab returned HTTP {status} for '{}': {}",
            context.graphql_url,
            truncate_for_error(&body, 500)
        ));
    }
    response
        .json::<GraphqlEnvelope<T>>()
        .map_err(|error| format!("failed to decode GitLab GraphQL response: {error}"))
}

fn gitlab_response(
    context: &GitlabContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::blocking::Response, InvocationResponse> {
    let url = format!("{}{}", context.api_url, path);
    let mut request = context
        .client
        .request(method, &url)
        .header("Accept", "application/json")
        .header("User-Agent", "AIHelper-gitlab-plugin");
    if let Some(token) = &context.token {
        request = request.header("PRIVATE-TOKEN", token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().map_err(|error| {
        InvocationResponse::error(
            "GITLAB_HTTP_FAILED",
            format!("request to '{url}' failed: {error}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "<failed to read response body>".to_owned());
        return Err(InvocationResponse::error(
            "GITLAB_API_FAILED",
            format!(
                "GitLab returned HTTP {status} for '{url}': {}",
                truncate_for_error(&body, 500)
            ),
        ));
    }
    Ok(response)
}

fn collect_job_trace(
    context: &GitlabContext,
    job_id: u64,
    grep: Option<&str>,
    warnings_only: bool,
    line_limit: Option<usize>,
    max_body_bytes: usize,
) -> Result<(Vec<TraceLine>, bool), InvocationResponse> {
    let path = format!(
        "/projects/{}/jobs/{job_id}/trace",
        context.project.encoded()
    );
    let response = gitlab_response(context, Method::GET, &path, None)?;
    if response
        .content_length()
        .is_some_and(|length| length > max_body_bytes as u64)
    {
        return Err(InvocationResponse::error(
            "GITLAB_RESPONSE_TOO_LARGE",
            format!("job trace exceeds --max-body-bytes {max_body_bytes}"),
        ));
    }

    let max_lines = line_limit.unwrap_or(usize::MAX);
    let grep_lower = grep.map(str::to_lowercase);
    let mut reader = BufReader::new(response);
    let mut body_bytes = 0usize;
    let mut line_bytes = Vec::new();
    let mut line_number = 0usize;
    let mut matches = Vec::new();
    loop {
        line_bytes.clear();
        let remaining = max_body_bytes.saturating_sub(body_bytes);
        let read = reader
            .by_ref()
            .take(remaining.saturating_add(1) as u64)
            .read_until(b'\n', &mut line_bytes)
            .map_err(|error| {
                InvocationResponse::error(
                    "GITLAB_RESPONSE_INVALID",
                    format!("failed to read job trace for job {job_id}: {error}"),
                )
            })?;
        if read == 0 {
            break;
        }
        body_bytes = body_bytes.saturating_add(read);
        if body_bytes > max_body_bytes {
            return Err(InvocationResponse::error(
                "GITLAB_RESPONSE_TOO_LARGE",
                format!("job trace exceeds --max-body-bytes {max_body_bytes}"),
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
        let text = strip_ansi_sequences(line);
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
        matches.push(TraceLine {
            line: line_number,
            text,
        });
    }
    Ok((matches, false))
}

fn get_issue(context: &GitlabContext, iid: u64) -> Result<IssueResponse, InvocationResponse> {
    let path = format!("/projects/{}/issues/{iid}", context.project.encoded());
    gitlab_json::<IssueResponse>(context, Method::GET, &path, None)
}

fn get_issue_comments(
    context: &GitlabContext,
    iid: u64,
    per_page: usize,
) -> Result<Vec<IssueNoteResponse>, InvocationResponse> {
    let path = format!(
        "/projects/{}/issues/{iid}/notes?per_page={per_page}&activity_filter=only_comments",
        context.project.encoded()
    );
    gitlab_json::<Vec<IssueNoteResponse>>(context, Method::GET, &path, None)
}

fn get_issue_designs_best_effort(
    context: &GitlabContext,
    iid: u64,
    first: usize,
) -> Result<Vec<IssueDesignResponse>, String> {
    let project_path = graphql_project_path(context)?;
    let envelope = gitlab_graphql::<IssueDesignsGraphqlData>(
        context,
        json!({
            "query": ISSUE_DESIGNS_QUERY,
            "variables": {
                "fullPath": project_path,
                "iid": iid.to_string(),
                "first": first,
            },
        }),
    )?;
    if !envelope.errors.is_empty() {
        let messages = envelope
            .errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("GitLab GraphQL designs query failed: {messages}"));
    }
    let Some(data) = envelope.data else {
        return Err("GitLab GraphQL designs query returned no data".to_owned());
    };
    let designs = data
        .project
        .and_then(|project| project.issue)
        .and_then(|issue| issue.design_collection)
        .and_then(|collection| collection.designs)
        .map(|connection| connection.nodes)
        .unwrap_or_default();
    Ok(designs)
}

fn graphql_project_path(context: &GitlabContext) -> Result<String, String> {
    if context.project.value.contains('/') {
        return Ok(context.project.value.clone());
    }
    let path = format!("/projects/{}", context.project.encoded());
    let project = gitlab_json::<GitlabProjectResponse>(context, Method::GET, &path, None).map_err(
        |error| {
            error
                .error_message
                .unwrap_or_else(|| "failed to resolve project path for GraphQL".to_owned())
        },
    )?;
    project.path_with_namespace.ok_or_else(|| {
        "GitLab project response did not include path_with_namespace for GraphQL designs query"
            .to_owned()
    })
}

fn create_issue_note(
    context: &GitlabContext,
    iid: u64,
    body: String,
) -> Result<IssueNoteResponse, InvocationResponse> {
    let path = format!("/projects/{}/issues/{iid}/notes", context.project.encoded());
    gitlab_json::<IssueNoteResponse>(context, Method::POST, &path, Some(json!({ "body": body })))
}

fn gitlab_issues_list_path(context: &GitlabContext, args: &IssuesArgs, per_page: usize) -> String {
    let mut query = vec![
        "scope=all".to_owned(),
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
        query.push(format!(
            "assignee_username={}",
            urlencoding::encode(assignee)
        ));
    }
    if let Some(author) = &args.author {
        query.push(format!("author_username={}", urlencoding::encode(author)));
    }
    if let Some(since) = &args.since {
        query.push(format!("updated_after={}", urlencoding::encode(since)));
    }
    if let Some(search) = &args.search {
        query.push(format!("search={}", urlencoding::encode(search)));
    }
    format!(
        "/projects/{}/issues?{}",
        context.project.encoded(),
        query.join("&")
    )
}

fn gitlab_state_event(state: &str) -> &'static str {
    match state {
        "closed" => "close",
        _ => "reopen",
    }
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

fn is_pipeline_terminal(status: &str) -> bool {
    matches!(
        status,
        "success" | "failed" | "canceled" | "skipped" | "manual"
    )
}

fn render_success<T: Serialize>(
    globals: &GlobalOptionsWire,
    output: &T,
    text_output: String,
) -> InvocationResponse {
    if globals.quiet {
        return InvocationResponse::ok(None);
    }
    if globals.json {
        match serde_json::to_string_pretty(output) {
            Ok(payload) => InvocationResponse::ok(Some(payload)),
            Err(error) => InvocationResponse::error(
                "JSON_SERIALIZATION_FAILED",
                format!("failed to serialize plugin output: {error}"),
            ),
        }
    } else {
        InvocationResponse::ok(Some(text_output))
    }
}

fn render_issues_text(issues: &[IssueResponse]) -> String {
    if issues.is_empty() {
        return String::new();
    }
    issues
        .iter()
        .map(|issue| {
            format!(
                "#{} {} {} {}",
                issue.iid,
                issue.state,
                issue.title,
                issue.web_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_comments_text(comments: &[IssueNoteResponse]) -> String {
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
                comment.id,
                comment
                    .author
                    .as_ref()
                    .and_then(|user| user.username.as_deref())
                    .unwrap_or("-"),
                first_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_issue_full_text(
    issue: &IssueResponse,
    comments: &[IssueNoteResponse],
    designs: &[IssueDesignResponse],
    warnings: &[String],
) -> String {
    let mut output = String::new();
    output.push_str(&format!("#{} {} {}\n", issue.iid, issue.state, issue.title));
    if let Some(web_url) = &issue.web_url {
        output.push_str(&format!("url: {web_url}\n"));
    }
    if let Some(author) = &issue.author {
        output.push_str(&format!("author: {}\n", render_user(author)));
    }
    let assignees = issue
        .assignees
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(render_user)
        .collect::<Vec<_>>()
        .join(", ");
    output.push_str(&format!(
        "assignees: {}\n",
        if assignees.is_empty() {
            "-"
        } else {
            assignees.as_str()
        }
    ));
    let labels = if issue.labels.is_empty() {
        "-".to_owned()
    } else {
        issue.labels.join(", ")
    };
    output.push_str(&format!("labels: {labels}\n"));
    output.push_str(&format!(
        "created: {}\nupdated: {}\nclosed: {}\n",
        issue.created_at.as_deref().unwrap_or("-"),
        issue.updated_at.as_deref().unwrap_or("-"),
        issue.closed_at.as_deref().unwrap_or("-")
    ));
    if let Some(description) = issue
        .description
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        output.push_str("\ndescription:\n");
        output.push_str(description);
        if !description.ends_with('\n') {
            output.push('\n');
        }
    }
    output.push_str(&format!("\ncomments ({}):\n", comments.len()));
    if comments.is_empty() {
        output.push_str("(none)\n");
    } else {
        output.push_str(&render_full_comments_text(comments));
    }
    output.push_str(&format!("\ndesigns ({}):\n", designs.len()));
    if designs.is_empty() {
        output.push_str("(none)\n");
    } else {
        output.push_str(&render_designs_text(designs));
    }
    if !warnings.is_empty() {
        output.push_str("\nwarnings:\n");
        for warning in warnings {
            output.push_str(&format!("- {warning}\n"));
        }
    }
    output
}

fn render_full_comments_text(comments: &[IssueNoteResponse]) -> String {
    comments
        .iter()
        .map(|comment| {
            let body = comment.body.as_deref().unwrap_or("");
            let mut rendered = format!(
                "- {} {} {}\n",
                comment.id,
                comment
                    .author
                    .as_ref()
                    .map(render_user)
                    .unwrap_or_else(|| "-".to_owned()),
                comment.created_at.as_deref().unwrap_or("-")
            );
            if !body.is_empty() {
                rendered.push_str(body);
                if !body.ends_with('\n') {
                    rendered.push('\n');
                }
            }
            rendered
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_designs_text(designs: &[IssueDesignResponse]) -> String {
    designs
        .iter()
        .map(|design| {
            format!(
                "{} {} {} {}",
                design.filename.as_deref().unwrap_or("-"),
                design.event.as_deref().unwrap_or("-"),
                design.notes_count.unwrap_or(0),
                design.image.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_user(user: &GitlabUser) -> String {
    if let Some(username) = &user.username {
        return format!("@{username}");
    }
    if let Some(name) = &user.name {
        return name.clone();
    }
    user.id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "-".to_owned())
}

fn render_releases_text(releases: &[ReleaseResponse]) -> String {
    if releases.is_empty() {
        return String::new();
    }
    releases
        .iter()
        .map(render_release_line)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_release_text(release: &ReleaseResponse) -> String {
    render_release_line(release) + "\n"
}

fn render_release_line(release: &ReleaseResponse) -> String {
    format!(
        "{} {} {}",
        release.tag_name,
        release.name.as_deref().unwrap_or("-"),
        release.released_at.as_deref().unwrap_or("-")
    )
}

fn render_pipelines_text(pipelines: &[PipelineResponse]) -> String {
    if pipelines.is_empty() {
        return String::new();
    }
    pipelines
        .iter()
        .map(|pipeline| {
            format!(
                "{} {} {} {}",
                pipeline.id,
                pipeline.r#ref.as_deref().unwrap_or("-"),
                pipeline.status,
                pipeline.web_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_jobs_text(jobs: &[JobResponse]) -> String {
    if jobs.is_empty() {
        return String::new();
    }
    jobs.iter()
        .map(|job| {
            format!(
                "{} {} {} {}",
                job.id,
                job.name,
                job.status,
                job.web_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn truncate_for_error(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect::<String>() + "..."
}

fn strip_ansi_sequences(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            output.push(ch);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    output
}

fn plugin_manual() -> PluginManual {
    PluginManual {
        plugin_name: PLUGIN_NAME.to_owned(),
        domain: DOMAIN.to_owned(),
        description: DESCRIPTION.to_owned(),
        commands: vec![
            ManualCommand {
                name: "project".to_owned(),
                summary: "Detect GitLab project context.".to_owned(),
                usage: "project [--project PATH_OR_ID] [--remote NAME] [--host URL] [--api-url URL] [--graphql-url URL] [--token TOKEN] [--use-git-credential]".to_owned(),
                examples: vec![manual_example("Inspect current GitLab project", &["project"])],
            },
            ManualCommand {
                name: "releases".to_owned(),
                summary: "List GitLab releases.".to_owned(),
                usage: "releases [--project PATH_OR_ID]".to_owned(),
                examples: vec![manual_example("List releases", &["releases"])],
            },
            ManualCommand {
                name: "release get".to_owned(),
                summary: "Get release metadata by tag.".to_owned(),
                usage: "release get <tag> [--project PATH_OR_ID]".to_owned(),
                examples: vec![manual_example("Inspect release v1.0.0", &["release", "get", "v1.0.0"])],
            },
            ManualCommand {
                name: "release create".to_owned(),
                summary: "Create a GitLab release for a tag.".to_owned(),
                usage: "release create <tag> [--name NAME] [--description TEXT|--description-file PATH] [--ref REF]".to_owned(),
                examples: vec![manual_example(
                    "Create release from description file",
                    &["release", "create", "v1.0.1", "--name", "v1.0.1", "--description-file", "RELEASE_NOTES.md"],
                )],
            },
            ManualCommand {
                name: "issues".to_owned(),
                summary: "List GitLab issues.".to_owned(),
                usage: "issues [--state opened|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT]".to_owned(),
                examples: vec![manual_example("List open bugs", &["issues", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue view".to_owned(),
                summary: "View issue metadata, optionally with comments and designs.".to_owned(),
                usage: "issue view <iid> [--full]".to_owned(),
                examples: vec![manual_example("Inspect issue", &["issue", "view", "42"])],
            },
            ManualCommand {
                name: "issue create".to_owned(),
                summary: "Create an issue.".to_owned(),
                usage: "issue create --title TITLE [--description TEXT|--description-file PATH] [--label LABEL ...] [--assignee-id ID ...]".to_owned(),
                examples: vec![manual_example("Create bug issue", &["issue", "create", "--title", "Fix build", "--description", "Build fails", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue update".to_owned(),
                summary: "Update issue fields.".to_owned(),
                usage: "issue update <iid> [--title TITLE] [--description TEXT|--description-file PATH] [--state opened|closed] [--label LABEL ...] [--assignee-id ID ...]".to_owned(),
                examples: vec![manual_example("Close issue via update", &["issue", "update", "42", "--state", "closed"])],
            },
            ManualCommand {
                name: "issue close".to_owned(),
                summary: "Close an issue, optionally after adding a comment.".to_owned(),
                usage: "issue close <iid> [--comment TEXT|--comment-file PATH]".to_owned(),
                examples: vec![manual_example("Close with comment", &["issue", "close", "42", "--comment", "Fixed in main"])],
            },
            ManualCommand {
                name: "issue comment".to_owned(),
                summary: "Add an issue comment.".to_owned(),
                usage: "issue comment <iid> --body TEXT|--body-file PATH".to_owned(),
                examples: vec![manual_example("Comment on issue", &["issue", "comment", "42", "--body", "I can reproduce this"])],
            },
            ManualCommand {
                name: "issue comments".to_owned(),
                summary: "List issue comments.".to_owned(),
                usage: "issue comments <iid>".to_owned(),
                examples: vec![manual_example("List comments", &["issue", "comments", "42"])],
            },
            ManualCommand {
                name: "pipelines".to_owned(),
                summary: "List GitLab pipelines.".to_owned(),
                usage: "pipelines [--branch BRANCH]".to_owned(),
                examples: vec![manual_example("List main pipelines", &["pipelines", "--branch", "main"])],
            },
            ManualCommand {
                name: "pipeline get".to_owned(),
                summary: "Get pipeline metadata.".to_owned(),
                usage: "pipeline get <pipeline-id>".to_owned(),
                examples: vec![manual_example("Inspect one pipeline", &["pipeline", "get", "42"])],
            },
            ManualCommand {
                name: "pipeline wait".to_owned(),
                summary: "Wait for pipeline completion.".to_owned(),
                usage: "pipeline wait <pipeline-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]".to_owned(),
                examples: vec![manual_example("Wait for one pipeline", &["pipeline", "wait", "42", "--fail-on-failure"])],
            },
            ManualCommand {
                name: "pipeline jobs".to_owned(),
                summary: "List jobs for a pipeline.".to_owned(),
                usage: "pipeline jobs <pipeline-id>".to_owned(),
                examples: vec![manual_example("Inspect pipeline jobs", &["pipeline", "jobs", "42"])],
            },
            ManualCommand {
                name: "job trace".to_owned(),
                summary: "Read or search a job trace.".to_owned(),
                usage: "job trace <job-id> [--grep TEXT] [--max-body-bytes BYTES]".to_owned(),
                examples: vec![manual_example("Search job trace", &["job", "trace", "7", "--grep", "warning"])],
            },
            ManualCommand {
                name: "job warnings".to_owned(),
                summary: "Extract warning-like lines from a job trace.".to_owned(),
                usage: "job warnings <job-id> [--max-body-bytes BYTES]".to_owned(),
                examples: vec![manual_example("List job warnings", &["job", "warnings", "7"])],
            },
        ],
        notes: vec![
            "GitLab-specific features live in this dynamic plugin; local Git commands stay in `ah git`.".to_owned(),
            "Project defaults to a GitLab path parsed from `origin`; override with --project group/project or numeric id.".to_owned(),
            "Use --host for self-managed GitLab, --api-url for nonstandard REST roots, and --graphql-url for a separately configured GraphQL endpoint.".to_owned(),
            "Authentication checks --token, GITLAB_TOKEN, then GL_TOKEN; use --use-git-credential to opt into git credential helper lookup.".to_owned(),
            "Use global --json for stable machine-readable output and --limit to cap releases, pipelines, or trace matches.".to_owned(),
            "Job traces default to an 8 MiB response budget; override with --max-body-bytes.".to_owned(),
        ],
    }
}

fn manual_example(description: &str, argv: &[&str]) -> ManualExample {
    ManualExample {
        description: description.to_owned(),
        argv: argv.iter().map(|item| (*item).to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{BufRead, BufReader, Read},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
    };

    use clap::{CommandFactory, Parser};

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

    #[test]
    fn parser_builds_command_tree() {
        let _ = GitlabCli::command();
    }

    #[test]
    fn parses_gitlab_project_refs() {
        assert_eq!(
            parse_project_ref("group/subgroup/tool.git")
                .expect("project should parse")
                .value,
            "group/subgroup/tool"
        );
        assert_eq!(
            parse_project_ref("123")
                .expect("project id should parse")
                .value,
            "123"
        );
        assert!(parse_project_ref("single").is_none());
    }

    #[test]
    fn parses_common_gitlab_remotes_for_custom_host() {
        let host = "https://gitlab.example.com";
        assert_eq!(
            parse_gitlab_remote_url("https://gitlab.example.com/group/tool.git", host)
                .expect("project should parse")
                .value,
            "group/tool"
        );
        assert_eq!(
            parse_gitlab_remote_url("git@gitlab.example.com:group/subgroup/tool.git", host)
                .expect("project should parse")
                .value,
            "group/subgroup/tool"
        );
        assert_eq!(
            parse_gitlab_remote_url("ssh://git@gitlab.example.com:2222/group/tool.git", host)
                .expect("project should parse")
                .value,
            "group/tool"
        );
    }

    #[test]
    fn rejects_non_matching_host_remote() {
        assert!(
            parse_gitlab_remote_url(
                "https://gitlab.other.example.com/group/tool.git",
                "https://gitlab.example.com"
            )
            .is_none()
        );
    }

    #[test]
    fn release_get_uses_encoded_project_and_private_token() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"{
                "tag_name": "v1.0.0",
                "name": "v1.0.0",
                "description": "notes",
                "created_at": "2026-05-07T00:00:00Z",
                "released_at": "2026-05-07T00:01:00Z",
                "upcoming_release": false,
                "assets": {"links": []}
            }"#,
        )]);

        let response = invoke_json(&[
            "--project",
            "group/subgroup/tool",
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
        assert_eq!(payload["command"], "gitlab.release.get");
        assert_eq!(payload["project"], "group/subgroup/tool");
        assert_eq!(payload["release"]["tag_name"], "v1.0.0");

        let request = only_request(&server);
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.path,
            "/projects/group%2Fsubgroup%2Ftool/releases/v1.0.0"
        );
        assert_eq!(request.header("private-token"), Some("secret-token"));
    }

    #[test]
    fn release_create_posts_expected_body() {
        let server = MockServer::new(vec![MockResponse::json(
            201,
            r#"{
                "tag_name": "v1.0.1",
                "name": "v1.0.1",
                "description": "release notes",
                "created_at": "2026-05-07T00:00:00Z",
                "released_at": "2026-05-07T00:01:00Z",
                "upcoming_release": false,
                "assets": {"links": []}
            }"#,
        )]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "release",
            "create",
            "v1.0.1",
            "--name",
            "v1.0.1",
            "--description",
            "release notes",
            "--ref",
            "main",
        ]);

        assert!(response.success, "{response:?}");
        let request = only_request(&server);
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/projects/group%2Ftool/releases");
        let body: Value = serde_json::from_str(&request.body).expect("body should be json");
        assert_eq!(body["tag_name"], "v1.0.1");
        assert_eq!(body["name"], "v1.0.1");
        assert_eq!(body["description"], "release notes");
        assert_eq!(body["ref"], "main");
    }

    #[test]
    fn pipelines_command_includes_ref_and_limit() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"[{
                "id": 42,
                "iid": 3,
                "project_id": 9,
                "sha": "abc123",
                "ref": "main",
                "status": "success",
                "source": "push",
                "web_url": "https://gitlab.example.com/group/tool/-/pipelines/42",
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z"
            }]"#,
        )]);

        let response = invoke_json_with_limit(
            &[
                "--project",
                "group/tool",
                "--api-url",
                &server.url(),
                "pipelines",
                "--branch",
                "main",
            ],
            Some(3),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["pipeline_count"], 1);
        let request = only_request(&server);
        assert_eq!(
            request.path,
            "/projects/group%2Ftool/pipelines?per_page=3&ref=main"
        );
    }

    #[test]
    fn pipeline_wait_polls_until_terminal() {
        let server = MockServer::new(vec![
            MockResponse::json(200, pipeline_json(42, "running")),
            MockResponse::json(200, pipeline_json(42, "success")),
        ]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "pipeline",
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
        assert_eq!(payload["command"], "gitlab.pipeline.wait");
        assert_eq!(payload["pipeline"]["status"], "success");
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn pipeline_wait_does_not_poll_after_deadline() {
        let server = MockServer::new(vec![MockResponse::json(200, pipeline_json(42, "running"))]);
        let started = Instant::now();

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "pipeline",
            "wait",
            "42",
            "--interval-secs",
            "60",
            "--timeout-secs",
            "1",
        ]);

        assert_eq!(
            response.error_code.as_deref(),
            Some("GITLAB_PIPELINE_TIMEOUT")
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn pipeline_jobs_decodes_list() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            r#"[{
                "id": 7,
                "name": "test",
                "status": "success",
                "stage": "test",
                "ref": "main",
                "allow_failure": false,
                "web_url": "https://gitlab.example.com/group/tool/-/jobs/7",
                "created_at": "2026-05-07T00:00:00Z",
                "started_at": "2026-05-07T00:00:10Z",
                "finished_at": "2026-05-07T00:01:00Z"
            }]"#,
        )]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "pipeline",
            "jobs",
            "42",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["job_count"], 1);
        assert_eq!(payload["jobs"][0]["name"], "test");
        assert_eq!(
            only_request(&server).path,
            "/projects/group%2Ftool/pipelines/42/jobs?per_page=100"
        );
    }

    #[test]
    fn job_trace_and_warnings_read_plain_text() {
        let server = MockServer::new(vec![MockResponse::bytes(
            200,
            "text/plain",
            "normal line\nwarning: deprecated config\n\u{1b}[1mwill be removed soon\u{1b}[0m\n"
                .as_bytes()
                .to_vec(),
        )]);

        let response = invoke_json_with_limit(
            &[
                "--project",
                "group/tool",
                "--api-url",
                &server.url(),
                "job",
                "warnings",
                "7",
            ],
            Some(10),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "gitlab.job.warnings");
        assert_eq!(payload["match_count"], 2);
        assert_eq!(
            payload["matches"][1]["text"], "will be removed soon",
            "ANSI escape sequences should be stripped"
        );
    }

    #[test]
    fn job_trace_rejects_oversized_body() {
        let server = MockServer::new(vec![MockResponse::bytes(
            200,
            "text/plain",
            b"0123456789\n".to_vec(),
        )]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "job",
            "trace",
            "7",
            "--max-body-bytes",
            "5",
        ]);

        assert_eq!(
            response.error_code.as_deref(),
            Some("GITLAB_RESPONSE_TOO_LARGE")
        );
    }

    #[test]
    fn issues_list_uses_filters_and_limit() {
        let server = MockServer::new(vec![MockResponse::json(
            200,
            &format!("[{}]", issue_json(21, "opened")),
        )]);

        let response = invoke_json_with_limit(
            &[
                "--project",
                "group/tool",
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
                "--search",
                "crash",
            ],
            Some(5),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "gitlab.issues");
        assert_eq!(payload["issue_count"], 1);
        let request = only_request(&server);
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.path,
            "/projects/group%2Ftool/issues?scope=all&state=all&per_page=5&labels=bug&assignee_username=bob&author_username=alice&updated_after=2026-05-07T00%3A00%3A00Z&search=crash"
        );
    }

    #[test]
    fn issue_view_uses_expected_request() {
        let server = MockServer::new(vec![MockResponse::json(200, issue_json(21, "opened"))]);
        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "issue",
            "view",
            "21",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "gitlab.issue.view");
        assert_eq!(payload["issue"]["iid"], 21);
        let request = only_request(&server);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/projects/group%2Ftool/issues/21");
    }

    #[test]
    fn issue_view_full_reads_comments_and_designs() {
        let server = MockServer::new(vec![
            MockResponse::json(200, issue_json(21, "opened")),
            MockResponse::json(200, &format!("[{}]", issue_note_json(101))),
            MockResponse::json(
                200,
                r#"{
                    "data": {
                        "project": {
                            "issue": {
                                "designCollection": {
                                    "designs": {
                                        "nodes": [{
                                            "id": "gid://gitlab/DesignManagement::Design/1",
                                            "filename": "mockup.png",
                                            "fullPath": "designs/mockup.png",
                                            "image": "https://gitlab.example.com/group/tool/uploads/designs/mockup.png",
                                            "imageV432x230": "https://gitlab.example.com/group/tool/uploads/designs/mockup.thumb.png",
                                            "notesCount": 2,
                                            "event": "NONE",
                                            "upstream_only": "ignored"
                                        }]
                                    }
                                }
                            }
                        }
                    }
                }"#,
            ),
        ]);

        let response = invoke_json_with_limit(
            &[
                "--project",
                "group/tool",
                "--api-url",
                &server.url(),
                "--graphql-url",
                &format!("{}/graphql", server.url()),
                "issue",
                "view",
                "21",
                "--full",
            ],
            Some(2),
        );

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["command"], "gitlab.issue.view");
        assert_eq!(payload["full"], true);
        assert_eq!(payload["comment_count"], 1);
        assert_eq!(payload["comments"][0]["body"], "I can reproduce this");
        assert_eq!(payload["design_count"], 1);
        assert_eq!(payload["designs"][0]["filename"], "mockup.png");
        assert!(payload["issue"].get("upstream_only").is_none());
        assert!(payload["comments"][0].get("upstream_only").is_none());
        assert!(payload["designs"][0].get("upstream_only").is_none());
        assert_eq!(
            payload["warnings"]
                .as_array()
                .expect("warnings array")
                .len(),
            0
        );

        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].path, "/projects/group%2Ftool/issues/21");
        assert_eq!(requests[1].method, "GET");
        assert_eq!(
            requests[1].path,
            "/projects/group%2Ftool/issues/21/notes?per_page=2&activity_filter=only_comments"
        );
        assert_eq!(requests[2].method, "POST");
        assert_eq!(requests[2].path, "/graphql");
        assert!(requests[2].body.contains("IssueDesigns"));
        assert!(requests[2].body.contains("\"fullPath\":\"group/tool\""));
        assert!(requests[2].body.contains("\"iid\":\"21\""));
        assert!(requests[2].body.contains("\"first\":2"));
    }

    #[test]
    fn issue_view_full_keeps_issue_when_design_query_fails() {
        let server = MockServer::new(vec![
            MockResponse::json(200, issue_json(21, "opened")),
            MockResponse::json(200, &format!("[{}]", issue_note_json(101))),
            MockResponse::json(
                200,
                r#"{"errors":[{"message":"Field 'designCollection' doesn't exist"}]}"#,
            ),
        ]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "--graphql-url",
            &format!("{}/graphql", server.url()),
            "issue",
            "view",
            "21",
            "--full",
        ]);

        assert!(response.success, "{response:?}");
        let payload = response_json(&response);
        assert_eq!(payload["full"], true);
        assert_eq!(payload["comment_count"], 1);
        assert_eq!(payload["design_count"], 0);
        assert_eq!(
            payload["warnings"][0],
            "GitLab GraphQL designs query failed: Field 'designCollection' doesn't exist"
        );
    }

    #[test]
    fn graphql_url_normalization_is_explicit_and_predictable() {
        assert_eq!(
            normalize_graphql_url(
                Some("https://proxy.example/graphql"),
                "https://gitlab.example/api/v4",
                "https://gitlab.example",
            )
            .expect("explicit GraphQL URL"),
            "https://proxy.example/graphql"
        );
        assert_eq!(
            normalize_graphql_url(
                None,
                "https://gitlab.example/api/v4",
                "https://gitlab.example",
            )
            .expect("standard API suffix"),
            "https://gitlab.example/api/graphql"
        );
        assert_eq!(
            normalize_graphql_url(None, "https://proxy.example/rest", "https://gitlab.example",)
                .expect("host fallback"),
            "https://gitlab.example/api/graphql"
        );
    }

    #[test]
    fn issue_create_and_update_send_expected_bodies() {
        let create_server =
            MockServer::new(vec![MockResponse::json(201, issue_json(21, "opened"))]);

        let create_response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &create_server.url(),
            "issue",
            "create",
            "--title",
            "Fix build",
            "--description",
            "Build fails",
            "--label",
            "bug",
            "--assignee-id",
            "10",
        ]);

        assert!(create_response.success, "{create_response:?}");
        let create_request = only_request(&create_server);
        assert_eq!(create_request.method, "POST");
        assert_eq!(create_request.path, "/projects/group%2Ftool/issues");
        let create_body: Value =
            serde_json::from_str(&create_request.body).expect("body should be json");
        assert_eq!(create_body["title"], "Fix build");
        assert_eq!(create_body["description"], "Build fails");
        assert_eq!(create_body["labels"], "bug");
        assert_eq!(create_body["assignee_ids"], json!([10]));

        let update_server =
            MockServer::new(vec![MockResponse::json(200, issue_json(21, "closed"))]);

        let update_response = invoke_json(&[
            "--project",
            "group/tool",
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
        assert_eq!(update_request.method, "PUT");
        assert_eq!(update_request.path, "/projects/group%2Ftool/issues/21");
        let update_body: Value =
            serde_json::from_str(&update_request.body).expect("body should be json");
        assert_eq!(update_body["state_event"], "close");
        assert_eq!(update_body["labels"], "fixed");
    }

    #[test]
    fn issue_close_comments_then_closes() {
        let server = MockServer::new(vec![
            MockResponse::json(201, issue_note_json(101)),
            MockResponse::json(200, issue_json(21, "closed")),
        ]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "issue",
            "close",
            "21",
            "--comment",
            "Fixed in main",
        ]);

        assert!(response.success, "{response:?}");
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, "/projects/group%2Ftool/issues/21/notes");
        assert_eq!(requests[1].method, "PUT");
        assert_eq!(requests[1].path, "/projects/group%2Ftool/issues/21");
        let close_body: Value =
            serde_json::from_str(&requests[1].body).expect("body should be json");
        assert_eq!(close_body["state_event"], "close");
    }

    #[test]
    fn issue_comment_and_comments_work() {
        let comment_server = MockServer::new(vec![MockResponse::json(201, issue_note_json(101))]);

        let comment_response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &comment_server.url(),
            "issue",
            "comment",
            "21",
            "--body",
            "I can reproduce this",
        ]);

        assert!(comment_response.success, "{comment_response:?}");
        let comment_payload = response_json(&comment_response);
        assert_eq!(comment_payload["command"], "gitlab.issue.comment");
        let comment_request = only_request(&comment_server);
        assert_eq!(
            comment_request.path,
            "/projects/group%2Ftool/issues/21/notes"
        );

        let list_server = MockServer::new(vec![MockResponse::json(
            200,
            &format!("[{}]", issue_note_json(101)),
        )]);

        let list_response = invoke_json_with_limit(
            &[
                "--project",
                "group/tool",
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
        assert_eq!(list_payload["command"], "gitlab.issue.comments");
        let list_request = only_request(&list_server);
        assert_eq!(
            list_request.path,
            "/projects/group%2Ftool/issues/21/notes?per_page=2&activity_filter=only_comments"
        );
    }

    #[test]
    fn gitlab_api_failure_has_stable_error_code() {
        let server = MockServer::new(vec![MockResponse::json(
            404,
            r#"{"message":"404 Project Not Found"}"#,
        )]);

        let response = invoke_json(&[
            "--project",
            "group/tool",
            "--api-url",
            &server.url(),
            "release",
            "get",
            "missing",
        ]);

        assert!(!response.success);
        assert_eq!(response.error_code.as_deref(), Some("GITLAB_API_FAILED"));
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
        let request = InvocationRequest {
            domain: DOMAIN.to_owned(),
            argv: argv.iter().map(|item| (*item).to_owned()).collect(),
            globals: GlobalOptionsWire {
                json: true,
                quiet: false,
                limit,
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

    fn pipeline_json(id: u64, status: &str) -> &'static str {
        let raw = format!(
            r#"{{
                "id": {id},
                "iid": 3,
                "project_id": 9,
                "sha": "abc123",
                "ref": "main",
                "status": "{status}",
                "source": "push",
                "web_url": "https://gitlab.example.com/group/tool/-/pipelines/{id}",
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z"
            }}"#
        );
        Box::leak(raw.into_boxed_str())
    }

    fn issue_json(iid: u64, state: &str) -> &'static str {
        let raw = format!(
            r#"{{
                "id": {iid},
                "iid": {iid},
                "project_id": 9,
                "title": "Fix build",
                "description": "Build fails",
                "state": "{state}",
                "web_url": "https://gitlab.example.com/group/tool/-/issues/{iid}",
                "author": {{"id": 1, "username": "alice", "name": "Alice"}},
                "assignees": [{{"id": 10, "username": "bob", "name": "Bob"}}],
                "labels": ["bug"],
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z",
                "closed_at": null,
                "upstream_only": "ignored"
            }}"#
        );
        Box::leak(raw.into_boxed_str())
    }

    fn issue_note_json(id: u64) -> &'static str {
        let raw = format!(
            r#"{{
                "id": {id},
                "body": "I can reproduce this",
                "author": {{"id": 10, "username": "bob", "name": "Bob"}},
                "created_at": "2026-05-07T00:00:00Z",
                "updated_at": "2026-05-07T00:01:00Z",
                "system": false,
                "upstream_only": "ignored"
            }}"#
        );
        Box::leak(raw.into_boxed_str())
    }

    fn only_request(server: &MockServer) -> CapturedRequest {
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        requests[0].clone()
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
                let deadline = Instant::now() + Duration::from_secs(5);
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
                while !handle.is_finished() {
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
