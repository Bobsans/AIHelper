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
        GithubCommand::Release(args) => execute_release(args, &context, globals),
        GithubCommand::Workflows => execute_workflows(&context, globals),
        GithubCommand::Workflow(args) => execute_workflow(args, &context, globals),
        GithubCommand::Runs(args) => execute_runs(args, &context, globals),
        GithubCommand::Run(args) => execute_run(args, &context, globals),
    }
}

fn execute_repo(context: &GithubContext, globals: &GlobalOptionsWire) -> InvocationResponse {
    let path = format!("/repos/{}/{}", context.repo.owner, context.repo.repo);
    let repo_response = github_json::<GithubRepoResponse>(context, Method::GET, &path, None);
    let (html_url, default_branch, private) = match repo_response {
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

        if start.elapsed() >= timeout {
            return InvocationResponse::error(
                "GITHUB_RUN_TIMEOUT",
                format!(
                    "workflow run {} did not complete within {} seconds",
                    args.run_id, args.timeout_secs
                ),
            );
        }
        thread::sleep(interval);
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
    let url = format!("{}{}", context.api_url, path);
    let mut request = context
        .client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "AIHelper-github-plugin");
    if let Some(token) = &context.token {
        request = request.bearer_auth(token);
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
        if items.len() > limit_value {
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
                name: "repo".to_owned(),
                summary: "Detect GitHub repository context.".to_owned(),
                usage: "repo [--repo OWNER/REPO] [--remote NAME] [--api-url URL] [--token TOKEN] [--use-git-credential]".to_owned(),
                examples: vec![manual_example("Inspect current GitHub repository", &["repo"])],
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
}
