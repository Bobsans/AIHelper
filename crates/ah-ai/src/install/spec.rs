//! What is being installed and where: the server spec, the scope, and the
//! project root the scope is relative to.

use super::*;

pub const DEFAULT_HTTP_URL: &str = "http://127.0.0.1:8787/mcp";

/// The directory the request is about.
///
/// `None` means the process directory: what the shell handed us, and the right
/// answer when `--cwd` was not given. It used to be the only answer, because
/// `--cwd` was applied by moving the whole process.
pub(crate) fn project_root(cwd: Option<&std::path::Path>) -> Result<PathBuf, AppError> {
    match cwd {
        Some(cwd) => Ok(cwd.to_path_buf()),
        None => std::env::current_dir().map_err(|source| AppError::cwd(PathBuf::from("."), source)),
    }
}

pub(crate) fn status_project_root(cwd: &std::path::Path) -> Option<PathBuf> {
    if let Some(root) = cwd.ancestors().find(|path| path.join(".git").exists()) {
        return Some(root.to_path_buf());
    }
    const MARKERS: [&str; 18] = [
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
        "opencode.json",
        "opencode.jsonc",
        ".mcp.json",
        ".claude",
        ".codex",
        ".gemini",
        ".cursor",
        ".vscode",
        ".github",
        ".opencode",
    ];
    MARKERS
        .iter()
        .any(|marker| cwd.join(marker).exists())
        .then(|| cwd.to_path_buf())
}

pub(crate) fn resolve_scope(target: &Target, requested: Option<Scope>) -> Result<Scope, AppError> {
    let scope = requested.unwrap_or(target.default_scope);
    target.require_scope(scope)?;
    Ok(scope)
}

pub(crate) fn stdio_spec() -> Result<ServerSpec, AppError> {
    let executable = std::env::current_exe().map_err(|source| {
        AppError::external(
            "AI_EXECUTABLE_UNRESOLVED",
            format!("unable to resolve the AIHelper executable path: {source}"),
        )
    })?;
    Ok(ServerSpec::Stdio {
        command: executable.display().to_string(),
        args: vec!["mcp".to_owned(), "serve".to_owned()],
    })
}

/// The server has no authentication or TLS, so only loopback endpoints are
/// ever written into an agent configuration.
pub(crate) fn http_spec(url: Option<&str>) -> Result<ServerSpec, AppError> {
    let url = url.unwrap_or(DEFAULT_HTTP_URL).to_owned();
    let parsed = reqwest::Url::parse(&url).map_err(|source| {
        AppError::external("AI_URL_INVALID", format!("invalid MCP URL {url}: {source}"))
    })?;
    if parsed.scheme() != "http"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.path() != "/mcp"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AppError::external(
            "AI_URL_INVALID",
            format!("{url} must be a plain HTTP loopback URL ending exactly in /mcp"),
        ));
    }
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    if !matches!(host, "127.0.0.1" | "::1" | "localhost") {
        return Err(AppError::external(
            "AI_URL_NOT_LOOPBACK",
            format!(
                "{url} is not a loopback endpoint; the AIHelper MCP server has no \
                 authentication or TLS and must not be exposed off-host"
            ),
        ));
    }
    Ok(ServerSpec::Http { url })
}

pub(crate) fn rules_block(manager: &PluginManager) -> String {
    let mut domains = manager
        .collect_plugin_manuals()
        .into_iter()
        .map(|manual| manual.domain)
        .collect::<Vec<_>>();
    domains.sort();
    domains.dedup();
    rules::render_block(&domains)
}

pub(crate) fn registrar_label(target: &Target) -> &'static str {
    match target.registrar {
        Registrar::Cli { .. } => "cli",
        Registrar::File => "file",
    }
}

pub(crate) fn config_path_for(
    target: &Target,
    scope: Scope,
    root: &std::path::Path,
) -> Result<Option<String>, AppError> {
    if target.probe == targets::ProbeKind::OpenCode {
        return Ok(Some(
            opencode_config::path(scope, root)?.display().to_string(),
        ));
    }
    match (target.registrar, target.json) {
        (Registrar::File, Some(config)) => {
            Ok(Some(config.path(scope, root)?.display().to_string()))
        }
        _ => Ok(None),
    }
}
