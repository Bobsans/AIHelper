use std::{
    env,
    ffi::c_char,
    fs,
    io::{Cursor, Read, Write},
    process::{Command, Stdio},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
    thread,
    time::{Duration, Instant},
};

use ah_plugin_api::{
    AH_PLUGIN_ABI_VERSION, AhPluginApiV1, GlobalOptionsWire, InvocationRequest, InvocationResponse,
    ManualCommand, ManualExample, PluginManual, c_ptr_to_string, free_c_string_ptr,
    manual_to_c_string, response_to_c_string,
};
use clap::{Args, Parser, Subcommand, error::ErrorKind};
use reqwest::{Method, blocking::Client};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use zip::ZipArchive;

const DOMAIN: &str = "github";
const PLUGIN_NAME: &str = "external-github";
const DESCRIPTION: &str = "GitHub Releases and Actions plugin (dynamic)";
const DEFAULT_API_URL: &str = "https://api.github.com";
const DEFAULT_REMOTE: &str = "origin";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const DEFAULT_WAIT_INTERVAL_SECS: u64 = 15;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 1800;

static PLUGIN_NAME_C: &[u8] = b"external-github\0";
static DOMAIN_C: &[u8] = b"github\0";
static DESCRIPTION_C: &[u8] = b"GitHub Releases and Actions plugin (dynamic)\0";

static PLUGIN_API_PTR: AtomicPtr<AhPluginApiV1> = AtomicPtr::new(ptr::null_mut());

#[derive(Debug, Parser)]
#[command(name = "github", about = "GitHub release and workflow helpers")]
struct GithubCli {
    #[command(flatten)]
    connection: GithubConnectionArgs,
    #[command(subcommand)]
    command: GithubCommand,
}

#[derive(Debug, Args, Clone)]
struct GithubConnectionArgs {
    #[arg(long, global = true, value_name = "OWNER/REPO")]
    repo: Option<String>,
    #[arg(long, global = true, default_value = DEFAULT_REMOTE, value_name = "NAME")]
    remote: String,
    #[arg(long, global = true, default_value = DEFAULT_API_URL, value_name = "URL")]
    api_url: String,
    #[arg(long, global = true, value_name = "TOKEN")]
    token: Option<String>,
    #[arg(long, global = true)]
    use_git_credential: bool,
    #[arg(long, global = true, default_value_t = DEFAULT_TIMEOUT_SECS, value_name = "SECONDS")]
    timeout_secs: u64,
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

#[derive(Debug, Args)]
struct IssuesArgs {
    #[arg(long, default_value = "open", value_parser = ["open", "closed", "all"])]
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
    #[command(about = "Get issue metadata")]
    Get(IssueNumberArgs),
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

#[derive(Debug, Args)]
struct IssueNumberArgs {
    number: u64,
}

#[derive(Debug, Args)]
struct CreateIssueArgs {
    #[arg(long)]
    title: String,
    #[arg(long, value_name = "TEXT")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
    body_file: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    assignees: Vec<String>,
}

#[derive(Debug, Args)]
struct UpdateIssueArgs {
    number: u64,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    body: Option<String>,
    #[arg(long, value_name = "PATH")]
    body_file: Option<String>,
    #[arg(long, value_parser = ["open", "closed"])]
    state: Option<String>,
    #[arg(long = "label", value_name = "LABEL")]
    labels: Vec<String>,
    #[arg(long = "assignee", value_name = "USER")]
    assignees: Vec<String>,
}

#[derive(Debug, Args)]
struct CloseIssueArgs {
    number: u64,
    #[arg(long, value_name = "TEXT")]
    comment: Option<String>,
    #[arg(long, value_name = "PATH")]
    comment_file: Option<String>,
}

#[derive(Debug, Args)]
struct CommentIssueArgs {
    number: u64,
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
    #[command(about = "List release assets by tag")]
    Assets(TagArgs),
    #[command(about = "Create a GitHub release for an existing or new tag")]
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
    title: Option<String>,
    #[arg(long, value_name = "TEXT")]
    notes: Option<String>,
    #[arg(long, value_name = "PATH")]
    notes_file: Option<String>,
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    draft: bool,
    #[arg(long)]
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

#[derive(Debug, Args)]
struct WorkflowRunArgs {
    workflow: String,
    #[arg(long, value_name = "REF")]
    r#ref: String,
    #[arg(long = "input", value_name = "KEY=VALUE")]
    inputs: Vec<String>,
}

#[derive(Debug, Args)]
struct RunsArgs {
    #[arg(long, value_name = "WORKFLOW")]
    workflow: Option<String>,
    #[arg(long, value_name = "BRANCH")]
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
    Warnings(RunIdArgs),
    #[command(about = "List workflow run artifacts")]
    Artifacts(RunIdArgs),
}

#[derive(Debug, Args)]
struct RunIdArgs {
    run_id: u64,
}

#[derive(Debug, Args)]
struct WaitRunArgs {
    run_id: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_INTERVAL_SECS, value_name = "SECONDS")]
    interval_secs: u64,
    #[arg(long, default_value_t = DEFAULT_WAIT_TIMEOUT_SECS, value_name = "SECONDS")]
    timeout_secs: u64,
    #[arg(long)]
    fail_on_failure: bool,
}

#[derive(Debug, Args)]
struct LogArgs {
    run_id: u64,
    #[arg(long)]
    grep: Option<String>,
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
struct GithubRepoResponse {
    full_name: Option<String>,
    html_url: Option<String>,
    default_branch: Option<String>,
    private: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GithubUser {
    login: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct GithubLabel {
    name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IssueResponse {
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
    #[serde(default)]
    pull_request: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IssueSearchResponse {
    items: Vec<IssueResponse>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct IssueOutput {
    command: &'static str,
    repository: String,
    issue: IssueResponse,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IssueCommentResponse {
    id: u64,
    body: Option<String>,
    html_url: Option<String>,
    user: Option<GithubUser>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct IssueCommentsOutput {
    command: &'static str,
    repository: String,
    number: u64,
    comment_count: usize,
    comments: Vec<IssueCommentResponse>,
}

#[derive(Debug, Serialize)]
struct IssueCommentOutput {
    command: &'static str,
    repository: String,
    number: u64,
    comment: IssueCommentResponse,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReleaseResponse {
    id: u64,
    tag_name: String,
    name: Option<String>,
    draft: bool,
    prerelease: bool,
    html_url: Option<String>,
    published_at: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ReleaseAsset {
    id: u64,
    name: String,
    size: u64,
    browser_download_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReleaseOutput {
    command: &'static str,
    repository: String,
    release: ReleaseResponse,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
struct WorkflowResponse {
    id: u64,
    name: String,
    path: String,
    state: String,
    html_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkflowsOutput {
    command: &'static str,
    repository: String,
    workflow_count: usize,
    workflows: Vec<WorkflowResponse>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize, Serialize, Clone)]
struct WorkflowRunResponse {
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

#[derive(Debug, Serialize)]
struct RunsOutput {
    command: &'static str,
    repository: String,
    workflow: Option<String>,
    branch: Option<String>,
    run_count: usize,
    runs: Vec<WorkflowRunResponse>,
}

#[derive(Debug, Serialize)]
struct RunOutput {
    command: &'static str,
    repository: String,
    run: WorkflowRunResponse,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
struct JobResponse {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct JobsOutput {
    command: &'static str,
    repository: String,
    run_id: u64,
    job_count: usize,
    jobs: Vec<JobResponse>,
}

#[derive(Debug, Clone, Serialize)]
struct LogLine {
    file: String,
    line: usize,
    text: String,
}

#[derive(Debug, Serialize)]
struct LogsOutput {
    command: &'static str,
    repository: String,
    run_id: u64,
    grep: Option<String>,
    match_count: usize,
    truncated: bool,
    matches: Vec<LogLine>,
}

#[derive(Debug, Serialize)]
struct ArtifactsOutput {
    command: &'static str,
    repository: String,
    run_id: u64,
    artifact_count: usize,
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactsListResponse {
    artifacts: Vec<ArtifactResponse>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ArtifactResponse {
    id: u64,
    name: String,
    size_in_bytes: u64,
    expired: bool,
    archive_download_url: Option<String>,
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_entry_v1() -> *const AhPluginApiV1 {
    let existing = PLUGIN_API_PTR.load(Ordering::Acquire);
    if !existing.is_null() {
        return existing.cast_const();
    }

    let created = Box::into_raw(Box::new(AhPluginApiV1 {
        abi_version: AH_PLUGIN_ABI_VERSION,
        plugin_name: PLUGIN_NAME_C.as_ptr().cast(),
        domain: DOMAIN_C.as_ptr().cast(),
        description: DESCRIPTION_C.as_ptr().cast(),
        invoke_json: ah_plugin_invoke_json,
        free_c_string: ah_plugin_free_c_string,
    }));

    match PLUGIN_API_PTR.compare_exchange(
        ptr::null_mut(),
        created,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => created.cast_const(),
        Err(existing) => {
            unsafe { drop(Box::from_raw(created)) };
            existing.cast_const()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ah_plugin_manual_json_v1() -> *mut c_char {
    manual_to_c_string(&plugin_manual())
}

unsafe extern "C" fn ah_plugin_invoke_json(request_json: *const c_char) -> *mut c_char {
    let response = invoke_from_raw(request_json);
    response_to_c_string(&response)
}

unsafe extern "C" fn ah_plugin_free_c_string(value: *mut c_char) {
    unsafe { free_c_string_ptr(value) };
}

fn invoke_from_raw(request_json: *const c_char) -> InvocationResponse {
    let request_json = match unsafe { c_ptr_to_string(request_json) } {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request pointer: {error}"),
            );
        }
    };

    let request = match serde_json::from_str::<InvocationRequest>(&request_json) {
        Ok(value) => value,
        Err(error) => {
            return InvocationResponse::error(
                "INVALID_ARGUMENT",
                format!("invalid request JSON: {error}"),
            );
        }
    };

    if request.domain != DOMAIN {
        return InvocationResponse::error(
            "INVALID_ARGUMENT",
            format!(
                "plugin domain mismatch: expected '{DOMAIN}', got '{}'",
                request.domain
            ),
        );
    }

    let parsed = match parse_args(&request.argv) {
        Ok(value) => value,
        Err(response) => return response,
    };

    execute(parsed, &request.globals)
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

    render_success(globals, &output, format!("{}\n", output.repository))
}

fn execute_issues(
    args: IssuesArgs,
    context: &GithubContext,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).max(1).min(100);
    let mut issues = if let Some(search) = &args.search {
        let path = github_issue_search_path(context, &args, search, per_page);
        match github_json::<IssueSearchResponse>(context, Method::GET, &path, None) {
            Ok(value) => value.items,
            Err(error) => return error,
        }
    } else {
        let path = github_issues_list_path(context, &args, per_page);
        match github_json::<Vec<IssueResponse>>(context, Method::GET, &path, None) {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
    issues.retain(|issue| issue.pull_request.is_none());
    let text = render_issues_text(&issues);
    render_success(
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
        IssueCommand::Get(args) => {
            let issue = match get_issue(context, args.number) {
                Ok(value) => value,
                Err(error) => return error,
            };
            render_success(
                globals,
                &IssueOutput {
                    command: "github.issue.get",
                    repository: context.repo.full_name(),
                    issue: issue.clone(),
                },
                render_issues_text(std::slice::from_ref(&issue)),
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
    render_success(
        globals,
        &IssueOutput {
            command: "github.issue.create",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
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
    render_success(
        globals,
        &IssueOutput {
            command: "github.issue.update",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
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
    if let Some(comment) = comment {
        if let Err(error) = create_issue_comment(context, args.number, comment) {
            return error;
        }
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
    render_success(
        globals,
        &IssueOutput {
            command: "github.issue.close",
            repository: context.repo.full_name(),
            issue: issue.clone(),
        },
        render_issues_text(std::slice::from_ref(&issue)),
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
    render_success(
        globals,
        &IssueCommentOutput {
            command: "github.issue.comment",
            repository: context.repo.full_name(),
            number: args.number,
            comment: comment.clone(),
        },
        render_comments_text(std::slice::from_ref(&comment)),
    )
}

fn issue_comments(
    context: &GithubContext,
    number: u64,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let per_page = globals.limit.unwrap_or(20).max(1).min(100);
    let path = format!(
        "/repos/{}/{}/issues/{number}/comments?per_page={per_page}",
        context.repo.owner, context.repo.repo
    );
    let comments = match github_json::<Vec<IssueCommentResponse>>(context, Method::GET, &path, None)
    {
        Ok(value) => value,
        Err(error) => return error,
    };
    render_success(
        globals,
        &IssueCommentsOutput {
            command: "github.issue.comments",
            repository: context.repo.full_name(),
            number,
            comment_count: comments.len(),
            comments: comments.clone(),
        },
        render_comments_text(&comments),
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
            let text = render_release_text(&release);
            render_success(
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
            let text = render_assets_text(&assets);
            render_success(
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
    let text = workflows
        .iter()
        .map(|workflow| format!("{} {} {}", workflow.id, workflow.state, workflow.path))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    render_success(
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
    let per_page = globals.limit.unwrap_or(10).max(1).min(100);
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
    let text = render_runs_text(&runs);
    render_success(
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
            let text = render_runs_text(std::slice::from_ref(&run));
            render_success(
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
        RunCommand::Logs(args) => run_logs(context, args.run_id, args.grep, globals, false),
        RunCommand::Warnings(args) => run_logs(context, args.run_id, None, globals, true),
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
    let body = json!({
        "tag_name": args.tag,
        "target_commitish": args.target,
        "name": args.title,
        "body": notes,
        "draft": args.draft,
        "prerelease": args.prerelease,
    });
    let path = format!(
        "/repos/{}/{}/releases",
        context.repo.owner, context.repo.repo
    );
    let release = match github_json::<ReleaseResponse>(context, Method::POST, &path, Some(body)) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let text = render_release_text(&release);
    render_success(
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

    let text = format!("dispatched {} on {}\n", args.workflow, args.r#ref);
    render_success(
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
            let text = render_runs_text(std::slice::from_ref(&run));
            return render_success(
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
        thread::sleep(interval.min(remaining));
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
    let text = jobs
        .iter()
        .map(|job| {
            format!(
                "{} {} {}",
                job.name,
                job.status,
                job.conclusion.as_deref().unwrap_or("-")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    render_success(
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
    globals: &GlobalOptionsWire,
    warnings_only: bool,
) -> InvocationResponse {
    let logs = match download_run_logs(context, run_id) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut matches = if warnings_only {
        warning_lines(&logs)
    } else if let Some(pattern) = &grep {
        grep_lines(&logs, pattern)
    } else {
        logs
    };
    let truncated = apply_limit(&mut matches, globals.limit);
    let text = matches
        .iter()
        .map(|line| format!("{}:{}: {}", line.file, line.line, line.text))
        .collect::<Vec<_>>()
        .join("\n")
        + if matches.is_empty() { "" } else { "\n" };
    render_success(
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
    let text = artifacts
        .iter()
        .map(|artifact| {
            format!(
                "{} {} expired={}",
                artifact.name, artifact.size_in_bytes, artifact.expired
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    render_success(
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

fn github_issues_list_path(context: &GithubContext, args: &IssuesArgs, per_page: usize) -> String {
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
    format!(
        "/repos/{}/{}/issues?{}",
        context.repo.owner,
        context.repo.repo,
        query.join("&")
    )
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
    let token = resolve_token(args);
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

    let remote_url = read_git_remote_url(&args.remote)?;
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

fn resolve_token(args: &GithubConnectionArgs) -> Option<String> {
    args.token
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| env_token("GITHUB_TOKEN"))
        .or_else(|| env_token("GH_TOKEN"))
        .or_else(|| {
            if args.use_git_credential {
                git_credential_token()
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

fn git_credential_token() -> Option<String> {
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
            .write_all(b"protocol=https\nhost=github.com\n\n")
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

fn github_json<T>(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<T, InvocationResponse>
where
    T: DeserializeOwned,
{
    let response = github_response(context, method, path, body)?;
    response.json::<T>().map_err(|error| {
        InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to decode GitHub response for '{path}': {error}"),
        )
    })
}

fn github_no_content(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<(), InvocationResponse> {
    let _response = github_response(context, method, path, body)?;
    Ok(())
}

fn github_response(
    context: &GithubContext,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<reqwest::blocking::Response, InvocationResponse> {
    let url = format!("{}{}", context.api_url, path);
    let mut request = context
        .client
        .request(method, &url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "AIHelper-github-plugin");
    if let Some(token) = &context.token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().map_err(|error| {
        InvocationResponse::error(
            "GITHUB_HTTP_FAILED",
            format!("request to '{url}' failed: {error}"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "<failed to read response body>".to_owned());
        return Err(InvocationResponse::error(
            "GITHUB_API_FAILED",
            format!(
                "GitHub returned HTTP {status} for '{url}': {}",
                truncate_for_error(&body, 500)
            ),
        ));
    }
    Ok(response)
}

fn download_run_logs(
    context: &GithubContext,
    run_id: u64,
) -> Result<Vec<LogLine>, InvocationResponse> {
    let path = format!(
        "/repos/{}/{}/actions/runs/{run_id}/logs",
        context.repo.owner, context.repo.repo
    );
    let response = github_response(context, Method::GET, &path, None)?;
    let bytes = response.bytes().map_err(|error| {
        InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to read log archive for run {run_id}: {error}"),
        )
    })?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        InvocationResponse::error(
            "GITHUB_RESPONSE_INVALID",
            format!("failed to open log archive for run {run_id}: {error}"),
        )
    })?;
    let mut lines = Vec::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| {
            InvocationResponse::error(
                "GITHUB_RESPONSE_INVALID",
                format!("failed to read log archive entry {index}: {error}"),
            )
        })?;
        if file.is_dir() {
            continue;
        }
        let file_name = file.name().to_owned();
        let mut content = String::new();
        if file.read_to_string(&mut content).is_err() {
            continue;
        }
        for (line_index, line) in content.lines().enumerate() {
            lines.push(LogLine {
                file: file_name.clone(),
                line: line_index + 1,
                text: strip_ansi_sequences(line),
            });
        }
    }
    Ok(lines)
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

fn grep_lines(lines: &[LogLine], pattern: &str) -> Vec<LogLine> {
    let needle = pattern.to_lowercase();
    lines
        .iter()
        .filter(|line| line.text.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}

fn warning_lines(lines: &[LogLine]) -> Vec<LogLine> {
    lines
        .iter()
        .filter(|line| is_warning_like(&line.text))
        .cloned()
        .collect()
}

fn is_warning_like(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("warning")
        || lower.contains("deprecated")
        || lower.contains("deprecation")
        || lower.contains("will be removed")
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
                issue.number,
                issue.state,
                issue.title,
                issue.html_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_comments_text(comments: &[IssueCommentResponse]) -> String {
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
                    .user
                    .as_ref()
                    .map(|user| user.login.as_str())
                    .unwrap_or("-"),
                first_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_release_text(release: &ReleaseResponse) -> String {
    format!(
        "{} draft={} prerelease={} assets={} {}\n",
        release.tag_name,
        release.draft,
        release.prerelease,
        release.assets.len(),
        release.html_url.as_deref().unwrap_or("")
    )
}

fn render_assets_text(assets: &[ReleaseAsset]) -> String {
    if assets.is_empty() {
        return String::new();
    }
    assets
        .iter()
        .map(|asset| {
            format!(
                "{} {} {}",
                asset.name,
                asset.size,
                asset.browser_download_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn render_runs_text(runs: &[WorkflowRunResponse]) -> String {
    if runs.is_empty() {
        return String::new();
    }
    runs.iter()
        .map(|run| {
            format!(
                "{} {} {} {} {} {}",
                run.id,
                run.name.as_deref().unwrap_or("-"),
                run.event,
                run.status,
                run.conclusion.as_deref().unwrap_or("-"),
                run.html_url.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn apply_limit<T>(items: &mut Vec<T>, limit: Option<usize>) -> bool {
    if let Some(limit_value) = limit {
        if limit_value > 0 && items.len() > limit_value {
            items.truncate(limit_value);
            return true;
        }
    }
    false
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
        match chars.peek() {
            Some(&'[') => {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(&']') => {
                chars.next();
                loop {
                    match chars.next() {
                        None | Some('\x07') => break,
                        Some('\u{1b}') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
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
                name: "repo".to_owned(),
                summary: "Detect GitHub repository context.".to_owned(),
                usage: "repo [--repo OWNER/REPO] [--remote NAME] [--api-url URL] [--token TOKEN] [--use-git-credential]".to_owned(),
                examples: vec![manual_example("Inspect current GitHub repository", &["repo"])],
            },
            ManualCommand {
                name: "issues".to_owned(),
                summary: "List GitHub issues.".to_owned(),
                usage: "issues [--state open|closed|all] [--label LABEL ...] [--assignee USER] [--author USER] [--since DATE] [--search TEXT]".to_owned(),
                examples: vec![manual_example("List open bugs", &["issues", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue get".to_owned(),
                summary: "Get issue metadata.".to_owned(),
                usage: "issue get <number>".to_owned(),
                examples: vec![manual_example("Inspect issue", &["issue", "get", "42"])],
            },
            ManualCommand {
                name: "issue create".to_owned(),
                summary: "Create an issue.".to_owned(),
                usage: "issue create --title TITLE [--body TEXT|--body-file PATH] [--label LABEL ...] [--assignee USER ...]".to_owned(),
                examples: vec![manual_example("Create bug issue", &["issue", "create", "--title", "Fix build", "--body", "Build fails", "--label", "bug"])],
            },
            ManualCommand {
                name: "issue update".to_owned(),
                summary: "Update issue fields.".to_owned(),
                usage: "issue update <number> [--title TITLE] [--body TEXT|--body-file PATH] [--state open|closed] [--label LABEL ...] [--assignee USER ...]".to_owned(),
                examples: vec![manual_example("Close issue via update", &["issue", "update", "42", "--state", "closed"])],
            },
            ManualCommand {
                name: "issue close".to_owned(),
                summary: "Close an issue, optionally after adding a comment.".to_owned(),
                usage: "issue close <number> [--comment TEXT|--comment-file PATH]".to_owned(),
                examples: vec![manual_example("Close with comment", &["issue", "close", "42", "--comment", "Fixed in main"])],
            },
            ManualCommand {
                name: "issue comment".to_owned(),
                summary: "Add an issue comment.".to_owned(),
                usage: "issue comment <number> --body TEXT|--body-file PATH".to_owned(),
                examples: vec![manual_example("Comment on issue", &["issue", "comment", "42", "--body", "I can reproduce this"])],
            },
            ManualCommand {
                name: "issue comments".to_owned(),
                summary: "List issue comments.".to_owned(),
                usage: "issue comments <number>".to_owned(),
                examples: vec![manual_example("List comments", &["issue", "comments", "42"])],
            },
            ManualCommand {
                name: "release get".to_owned(),
                summary: "Get release metadata by tag.".to_owned(),
                usage: "release get <tag> [--repo OWNER/REPO]".to_owned(),
                examples: vec![manual_example("Inspect release v0.3.0", &["release", "get", "v0.3.0"])],
            },
            ManualCommand {
                name: "release assets".to_owned(),
                summary: "List release assets by tag.".to_owned(),
                usage: "release assets <tag> [--repo OWNER/REPO]".to_owned(),
                examples: vec![manual_example("List release assets", &["release", "assets", "v0.3.0"])],
            },
            ManualCommand {
                name: "release create".to_owned(),
                summary: "Create a GitHub Release for a tag.".to_owned(),
                usage: "release create <tag> [--title TITLE] [--notes TEXT|--notes-file PATH] [--target REF] [--draft] [--prerelease]".to_owned(),
                examples: vec![manual_example(
                    "Create release from notes file",
                    &["release", "create", "v0.3.1", "--title", "v0.3.1", "--notes-file", "RELEASE_NOTES.md"],
                )],
            },
            ManualCommand {
                name: "workflows".to_owned(),
                summary: "List GitHub Actions workflows.".to_owned(),
                usage: "workflows [--repo OWNER/REPO]".to_owned(),
                examples: vec![manual_example("List workflows", &["workflows"])],
            },
            ManualCommand {
                name: "workflow run".to_owned(),
                summary: "Dispatch a workflow by id or file name.".to_owned(),
                usage: "workflow run <workflow> --ref <ref> [--input KEY=VALUE ...]".to_owned(),
                examples: vec![manual_example(
                    "Run release workflow on main",
                    &["workflow", "run", "release.yml", "--ref", "main"],
                )],
            },
            ManualCommand {
                name: "runs".to_owned(),
                summary: "List workflow runs.".to_owned(),
                usage: "runs [--workflow WORKFLOW] [--branch BRANCH]".to_owned(),
                examples: vec![manual_example(
                    "List release workflow runs",
                    &["runs", "--workflow", "release.yml", "--branch", "main"],
                )],
            },
            ManualCommand {
                name: "run get".to_owned(),
                summary: "Get workflow run metadata.".to_owned(),
                usage: "run get <run-id>".to_owned(),
                examples: vec![manual_example("Inspect one run", &["run", "get", "25451983278"])],
            },
            ManualCommand {
                name: "run wait".to_owned(),
                summary: "Wait for workflow run completion.".to_owned(),
                usage: "run wait <run-id> [--interval-secs SECONDS] [--timeout-secs SECONDS] [--fail-on-failure]".to_owned(),
                examples: vec![manual_example("Wait for one run", &["run", "wait", "25451983278", "--fail-on-failure"])],
            },
            ManualCommand {
                name: "run jobs".to_owned(),
                summary: "List jobs for a workflow run.".to_owned(),
                usage: "run jobs <run-id>".to_owned(),
                examples: vec![manual_example("Inspect run jobs", &["run", "jobs", "25451983278"])],
            },
            ManualCommand {
                name: "run logs".to_owned(),
                summary: "Search workflow run logs.".to_owned(),
                usage: "run logs <run-id> [--grep TEXT]".to_owned(),
                examples: vec![manual_example(
                    "Search logs for Node warning",
                    &["run", "logs", "25451983278", "--grep", "Node.js 20 actions are deprecated"],
                )],
            },
            ManualCommand {
                name: "run warnings".to_owned(),
                summary: "Extract warning-like lines from workflow run logs.".to_owned(),
                usage: "run warnings <run-id>".to_owned(),
                examples: vec![manual_example("List run warnings", &["run", "warnings", "25451983278"])],
            },
            ManualCommand {
                name: "run artifacts".to_owned(),
                summary: "List workflow run artifacts.".to_owned(),
                usage: "run artifacts <run-id>".to_owned(),
                examples: vec![manual_example("List run artifacts", &["run", "artifacts", "25451983278"])],
            },
        ],
        notes: vec![
            "GitHub-specific features live in this dynamic plugin; local Git commands stay in `ah git`.".to_owned(),
            "Repository defaults to GitHub owner/repo parsed from `origin`; override with --repo OWNER/REPO.".to_owned(),
            "Authentication checks --token, GITHUB_TOKEN, then GH_TOKEN; use --use-git-credential to opt into git credential helper lookup.".to_owned(),
            "Use global --json for stable machine-readable output and --limit to cap runs/log matches.".to_owned(),
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
        io::{BufRead, BufReader},
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
            strip_ansi_sequences("\u{1b}[1mDownloaded\u{1b}[0m"),
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
