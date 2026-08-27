//! Finding a usable `psql`, and reporting why a candidate was rejected.
//!
//! Three sources in a fixed order - an explicit path, the managed download,
//! `PATH` - and every one of them is version-checked before it is accepted,
//! because a `psql` older than 14 does not have the JSON output this plugin
//! reads. A rejected candidate keeps its reason so `tool status` can show it.

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct ToolContext {
    pub(crate) psql_path: PathBuf,
    pub(crate) bin_dir: PathBuf,
    pub(crate) version: ToolVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ToolSource {
    Explicit,
    Env,
    Configured,
    ManagedCache,
    SystemPath,
}

impl ToolSource {
    fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Env => "env",
            Self::Configured => "configured",
            Self::ManagedCache => "managed-cache",
            Self::SystemPath => "system-path",
        }
    }

    fn is_explicit_intent(self) -> bool {
        matches!(self, Self::Explicit | Self::Env | Self::Configured)
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolVersion {
    pub(crate) raw: String,
    #[schemars(range(min = 1))]
    pub(crate) major: Option<u32>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidateStatus {
    pub(crate) source: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) psql_path: Option<PathBuf>,
    pub(crate) bin_dir: Option<PathBuf>,
    pub(crate) version_raw: Option<String>,
    #[schemars(range(min = 1))]
    pub(crate) version_major: Option<u32>,
    pub(crate) accepted: bool,
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolStatusOutput {
    pub(crate) command: &'static str,
    pub(crate) available: bool,
    pub(crate) selected: Option<CandidateStatus>,
    pub(crate) candidates: Vec<CandidateStatus>,
    pub(crate) target_version: &'static str,
    #[schemars(range(min = 1))]
    pub(crate) minimum_major: u32,
    pub(crate) cache_dir: Option<PathBuf>,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) remediation: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolDownloadOutput {
    pub(crate) command: &'static str,
    pub(crate) version: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) cache_path: PathBuf,
    pub(crate) bin_dir: PathBuf,
    pub(crate) psql_path: PathBuf,
    pub(crate) downloaded: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolUseOutput {
    pub(crate) command: &'static str,
    pub(crate) path: PathBuf,
    pub(crate) psql_path: PathBuf,
    pub(crate) version: ToolVersion,
    pub(crate) config_path: PathBuf,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ToolCleanupOutput {
    pub(crate) command: &'static str,
    pub(crate) removed: Vec<PathBuf>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MissingToolOutput {
    pub(crate) available: bool,
    pub(crate) required: String,
    pub(crate) target: &'static str,
    pub(crate) searched: Vec<CandidateStatus>,
    pub(crate) remediation: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ToolConfig {
    pub(crate) version: u32,
    pub(crate) path: Option<PathBuf>,
}

pub(crate) fn execute_tool(
    args: ToolArgs,
    resolver_args: &ToolResolverArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match args.command {
        ToolCommand::Status => execute_tool_status(resolver_args, globals),
        ToolCommand::Download(args) => execute_tool_download(args, globals),
        ToolCommand::Use(args) => execute_tool_use(args, globals),
        ToolCommand::Cleanup(args) => execute_tool_cleanup(args, globals),
    }
}

pub(crate) fn execute_tool_status(
    resolver_args: &ToolResolverArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let candidates = inspect_tool_candidates(resolver_args);
    let selected = candidates
        .iter()
        .find(|candidate| candidate.accepted)
        .cloned();
    let output = ToolStatusOutput {
        command: "postgres.tool.status",
        available: selected.is_some(),
        selected,
        candidates,
        target_version: DEFAULT_POSTGRES_VERSION,
        minimum_major: MIN_POSTGRES_MAJOR,
        cache_dir: postgres_cache_root().ok(),
        config_path: tool_config_path().ok(),
        remediation: Some("ah postgres tool download".to_owned()),
    };

    render::render_success(
        globals,
        &output,
        render_tool_status_text(&output, TextFormatter::stdout()),
    )
}

pub(crate) fn execute_tool_download(
    args: ToolDownloadArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    match download_managed_tool(&args.version, args.force, args.download_timeout_secs) {
        Ok(output) => render::render_success(
            globals,
            &output,
            render_tool_download_text(&output, TextFormatter::stdout()),
        ),
        Err(error) => error,
    }
}

pub(crate) fn execute_tool_use(
    args: ToolUseArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let candidate = match evaluate_candidate(ToolSource::Configured, args.path.clone()) {
        CandidateEvaluation::Accepted(context) => context,
        CandidateEvaluation::Rejected(candidate) => {
            return InvocationResponse::error(
                "POSTGRES_TOOL_UNAVAILABLE",
                format!(
                    "configured PostgreSQL tool path '{}' is not usable: {}",
                    args.path.display(),
                    candidate
                        .reason
                        .unwrap_or_else(|| "unknown validation failure".to_owned())
                ),
            );
        }
    };

    let config_path = match tool_config_path() {
        Ok(path) => path,
        Err(error) => return error,
    };
    if let Err(error) = write_tool_config(&ToolConfig {
        version: SETTINGS_VERSION,
        path: Some(candidate.bin_dir.clone()),
    }) {
        return error;
    }

    let output = ToolUseOutput {
        command: "postgres.tool.use",
        path: candidate.bin_dir.clone(),
        psql_path: candidate.psql_path.clone(),
        version: candidate.version.clone(),
        config_path,
    };
    let formatter = TextFormatter::stdout();
    render::render_success(
        globals,
        &output,
        format!(
            "{} {} {}\n",
            formatter.paint(TextStyle::Success, "using PostgreSQL toolchain"),
            formatter.paint(TextStyle::Muted, "at"),
            formatter.paint(TextStyle::Key, output.path.display())
        ),
    )
}

pub(crate) fn execute_tool_cleanup(
    args: ToolCleanupArgs,
    globals: &GlobalOptionsWire,
) -> InvocationResponse {
    let cache_root = match postgres_cache_root() {
        Ok(path) => path,
        Err(error) => return error,
    };
    let mut removed = Vec::new();
    if let Some(version) = args.version {
        let path = cache_root.join(version);
        if path.exists() {
            if let Err(error) = remove_cache_dir(&cache_root, &path) {
                return error;
            }
            removed.push(path);
        }
    } else if cache_root.exists() {
        let entries = match fs::read_dir(&cache_root) {
            Ok(entries) => entries,
            Err(error) => {
                return InvocationResponse::error(
                    "POSTGRES_TOOL_CLEANUP_FAILED",
                    format!(
                        "failed to read PostgreSQL tool cache '{}': {error}",
                        cache_root.display()
                    ),
                );
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(value) => value,
                Err(error) => {
                    return InvocationResponse::error(
                        "POSTGRES_TOOL_CLEANUP_FAILED",
                        format!("failed to read PostgreSQL tool cache entry: {error}"),
                    );
                }
            };
            let path = entry.path();
            if path.is_dir() {
                if let Err(error) = remove_cache_dir(&cache_root, &path) {
                    return error;
                }
                removed.push(path);
            }
        }
    }

    let output = ToolCleanupOutput {
        command: "postgres.tool.cleanup",
        removed,
    };
    let formatter = TextFormatter::stdout();
    let text = if output.removed.is_empty() {
        format!(
            "{}\n",
            formatter.paint(
                TextStyle::Muted,
                "no managed PostgreSQL tool cache entries removed"
            )
        )
    } else {
        format!(
            "{} {} {}\n",
            formatter.paint(TextStyle::Success, "removed"),
            formatter.paint(TextStyle::Key, output.removed.len()),
            formatter.paint(
                TextStyle::Muted,
                format!(
                    "managed PostgreSQL tool cache entr{}",
                    if output.removed.len() == 1 {
                        "y"
                    } else {
                        "ies"
                    }
                )
            ),
        )
    };
    render::render_success(globals, &output, text)
}

pub(crate) fn resolve_operational_tool(
    args: &ToolResolverArgs,
) -> Result<ToolContext, InvocationResponse> {
    let candidates = inspect_tool_candidates(args);
    if let Some(first) = candidates.first()
        && source_from_label(first.source)
            .map(ToolSource::is_explicit_intent)
            .unwrap_or(false)
        && !first.accepted
    {
        return Err(missing_tool_response(candidates));
    }

    if let Some(candidate) = candidates.iter().find(|candidate| candidate.accepted) {
        return accepted_candidate_to_context(candidate)
            .ok_or_else(|| missing_tool_response(candidates));
    }

    if args.ensure_tool {
        download_managed_tool(
            DEFAULT_POSTGRES_VERSION,
            false,
            DEFAULT_DOWNLOAD_TIMEOUT_SECS,
        )?;
        let candidates = inspect_tool_candidates(args);
        if let Some(candidate) = candidates.iter().find(|candidate| candidate.accepted) {
            return accepted_candidate_to_context(candidate)
                .ok_or_else(|| missing_tool_response(candidates));
        }
        return Err(missing_tool_response(candidates));
    }

    Err(missing_tool_response(candidates))
}

pub(crate) fn missing_tool_response(candidates: Vec<CandidateStatus>) -> InvocationResponse {
    let output = MissingToolOutput {
        available: false,
        required: format!("psql >= {MIN_POSTGRES_MAJOR}"),
        target: DEFAULT_POSTGRES_VERSION,
        searched: candidates,
        remediation: vec![
            "ah postgres tool download".to_owned(),
            "ah postgres tool use --path PATH".to_owned(),
        ],
    };
    let details = serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_owned());
    InvocationResponse::error(
        "POSTGRES_TOOL_UNAVAILABLE",
        format!(
            "PostgreSQL toolchain is not available. Run: ah postgres tool download. Or set a path: ah postgres tool use --path PATH. Details: {details}"
        ),
    )
}

pub(crate) enum CandidateEvaluation {
    Accepted(ToolContext),
    Rejected(CandidateStatus),
}

pub(crate) fn inspect_tool_candidates(args: &ToolResolverArgs) -> Vec<CandidateStatus> {
    let mut candidates = Vec::new();
    if let Some(path) = &args.tool_path {
        candidates.push(candidate_status(ToolSource::Explicit, path.clone()));
        return candidates;
    }
    if let Some(path) = env::var_os(AH_POSTGRES_TOOL_PATH)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        candidates.push(candidate_status(ToolSource::Env, path));
        return candidates;
    }
    if let Ok(config) = read_tool_config()
        && let Some(path) = config.path
    {
        candidates.push(candidate_status(ToolSource::Configured, path));
        return candidates;
    }
    if let Ok(path) = managed_tool_path(DEFAULT_POSTGRES_VERSION)
        && path.exists()
    {
        candidates.push(candidate_status(ToolSource::ManagedCache, path));
    }
    if let Some(path) = find_psql_in_path() {
        candidates.push(candidate_status(ToolSource::SystemPath, path));
    }
    candidates
}

pub(crate) fn candidate_status(source: ToolSource, path: PathBuf) -> CandidateStatus {
    match evaluate_candidate(source, path.clone()) {
        CandidateEvaluation::Accepted(context) => CandidateStatus {
            source: source.label(),
            path,
            psql_path: Some(context.psql_path),
            bin_dir: Some(context.bin_dir),
            version_raw: Some(context.version.raw),
            version_major: context.version.major,
            accepted: true,
            reason: None,
        },
        CandidateEvaluation::Rejected(status) => status,
    }
}

pub(crate) fn evaluate_candidate(source: ToolSource, path: PathBuf) -> CandidateEvaluation {
    let (psql_path, bin_dir) = match resolve_psql_path(&path) {
        Ok(value) => value,
        Err(reason) => {
            return CandidateEvaluation::Rejected(CandidateStatus {
                source: source.label(),
                path,
                psql_path: None,
                bin_dir: None,
                version_raw: None,
                version_major: None,
                accepted: false,
                reason: Some(reason),
            });
        }
    };

    match psql_version(&psql_path, &bin_dir) {
        Ok(version) => {
            let accepted = version.major.unwrap_or(0) >= MIN_POSTGRES_MAJOR;
            if accepted {
                CandidateEvaluation::Accepted(ToolContext {
                    psql_path,
                    bin_dir,
                    version,
                })
            } else {
                CandidateEvaluation::Rejected(CandidateStatus {
                    source: source.label(),
                    path,
                    psql_path: Some(psql_path),
                    bin_dir: Some(bin_dir),
                    version_raw: Some(version.raw),
                    version_major: version.major,
                    accepted: false,
                    reason: Some(format!("psql major version is below {MIN_POSTGRES_MAJOR}")),
                })
            }
        }
        Err(reason) => CandidateEvaluation::Rejected(CandidateStatus {
            source: source.label(),
            path,
            psql_path: Some(psql_path),
            bin_dir: Some(bin_dir),
            version_raw: None,
            version_major: None,
            accepted: false,
            reason: Some(reason),
        }),
    }
}

pub(crate) fn accepted_candidate_to_context(candidate: &CandidateStatus) -> Option<ToolContext> {
    if !candidate.accepted {
        return None;
    }
    let psql_path = candidate.psql_path.clone()?;
    let bin_dir = candidate.bin_dir.clone()?;
    let version = ToolVersion {
        raw: candidate.version_raw.clone()?,
        major: candidate.version_major,
    };
    Some(ToolContext {
        psql_path,
        bin_dir,
        version,
    })
}

pub(crate) fn source_from_label(label: &str) -> Option<ToolSource> {
    match label {
        "explicit" => Some(ToolSource::Explicit),
        "env" => Some(ToolSource::Env),
        "configured" => Some(ToolSource::Configured),
        "managed-cache" => Some(ToolSource::ManagedCache),
        "system-path" => Some(ToolSource::SystemPath),
        _ => None,
    }
}

pub(crate) fn resolve_psql_path(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    if path.is_file() {
        let bin_dir = path
            .parent()
            .ok_or_else(|| {
                format!(
                    "tool executable '{}' has no parent directory",
                    path.display()
                )
            })?
            .to_path_buf();
        return Ok((path.to_path_buf(), bin_dir));
    }
    if !path.exists() {
        return Err(format!("path does not exist: {}", path.display()));
    }
    if !path.is_dir() {
        return Err(format!(
            "path is not a file or directory: {}",
            path.display()
        ));
    }

    let candidates = [
        path.join(psql_exe_name()),
        path.join("pgsql").join("bin").join(psql_exe_name()),
        path.join("bin").join(psql_exe_name()),
    ];
    for psql_path in candidates {
        if psql_path.is_file() {
            let bin_dir = psql_path
                .parent()
                .ok_or_else(|| {
                    format!(
                        "tool executable '{}' has no parent directory",
                        psql_path.display()
                    )
                })?
                .to_path_buf();
            return Ok((psql_path, bin_dir));
        }
    }
    Err(format!(
        "no {} found under '{}'",
        psql_exe_name(),
        path.display()
    ))
}

pub(crate) fn psql_version(psql_path: &Path, bin_dir: &Path) -> Result<ToolVersion, String> {
    let output = noninteractive_command(psql_path)
        .arg("--version")
        .env("PATH", prepend_path_env(bin_dir))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            format!(
                "failed to execute '{} --version': {error}",
                psql_path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "'{} --version' failed with exit code {:?}: {}",
            psql_path.display(),
            output.status.code(),
            render::truncate_for_error(&String::from_utf8_lossy(&output.stderr), 400)
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    Ok(ToolVersion {
        major: parse_psql_major(&raw),
        raw,
    })
}

pub(crate) fn parse_psql_major(raw: &str) -> Option<u32> {
    raw.split_whitespace().find_map(|part| {
        let first = part.split('.').next()?;
        first.parse::<u32>().ok()
    })
}

pub(crate) fn find_psql_in_path() -> Option<PathBuf> {
    ah_platform::exec::find_executable("psql", &[])
}

pub(crate) fn psql_exe_name() -> &'static str {
    if cfg!(windows) { "psql.exe" } else { "psql" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_psql_major_version() {
        assert_eq!(parse_psql_major("psql (PostgreSQL) 18.4"), Some(18));
        assert_eq!(parse_psql_major("psql (PostgreSQL) 14.12"), Some(14));
    }
}
