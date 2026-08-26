use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    System,
    Local,
    Project,
    User,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Local => "local",
            Self::Project => "project",
            Self::User => "user",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "local" => Some(Self::Local),
            "project" => Some(Self::Project),
            "user" => Some(Self::User),
            _ => None,
        }
    }

    /// Local and project scopes both live next to the repository.
    pub fn is_project_local(self) -> bool {
        matches!(self, Self::Local | Self::Project)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Http,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSpec {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}

impl ServerSpec {
    pub fn transport(&self) -> Transport {
        match self {
            Self::Stdio { .. } => Transport::Stdio,
            Self::Http { .. } => Transport::Http,
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Http { url } => Some(url),
            Self::Stdio { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliStyle {
    Claude,
    Codex,
    Gemini,
}

/// An agent configuration expressed as a JSON document with one server map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonConfig {
    /// Server map key: `mcpServers` for most agents, `servers` for Copilot.
    pub key: &'static str,
    /// Path relative to the project root, when the agent has a project scope.
    pub project_path: Option<&'static str>,
    /// Path relative to the home directory, when the agent has a user scope.
    pub user_path: Option<&'static str>,
}

impl JsonConfig {
    pub fn path(&self, scope: Scope, project_root: &Path) -> Result<PathBuf, AppError> {
        let relative = if scope.is_project_local() {
            self.project_path
        } else {
            self.user_path
        };
        let relative = relative.ok_or_else(|| {
            AppError::external(
                "AI_TARGET_SCOPE_UNSUPPORTED",
                format!("no configuration path for the {} scope", scope.as_str()),
            )
        })?;
        if scope.is_project_local() {
            Ok(join_relative(project_root, relative))
        } else {
            Ok(join_relative(&home_dir()?, relative))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registrar {
    /// The agent owns its configuration; AIHelper calls its CLI.
    Cli {
        program: &'static str,
        style: CliStyle,
    },
    /// The agent has no CLI; AIHelper merges its JSON configuration.
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// Claude's own layout, including its per-project server map.
    ClaudeConfig,
    /// `codex mcp list --json`.
    CodexList,
    /// A plain JSON configuration read without ever writing it.
    Json,
    /// OpenCode's comment-preserving JSONC configuration.
    OpenCode,
}

#[derive(Debug)]
pub struct Target {
    pub name: &'static str,
    pub scopes: &'static [Scope],
    pub status_rules_scopes: &'static [Scope],
    pub status_mcp_scopes: &'static [Scope],
    pub default_scope: Scope,
    /// Set when the agent keeps MCP servers in exactly one scope regardless of
    /// where its rules file lives.
    pub forced_mcp_scope: Option<Scope>,
    pub rules_file: &'static str,
    pub user_dir: &'static str,
    pub registrar: Registrar,
    pub probe: ProbeKind,
    pub json: Option<JsonConfig>,
}

pub const SERVER_NAME: &str = "aihelper";

/// Server names earlier AIHelper releases registered. They are invisible to the
/// current probe, so install and uninstall clean them up explicitly rather than
/// leaving a duplicate registration behind.
pub const LEGACY_SERVER_NAMES: &[&str] = &["ah"];

#[cfg(windows)]
const CODEX_STATUS_MCP_SCOPES: &[Scope] = &[Scope::User, Scope::Project];
#[cfg(not(windows))]
const CODEX_STATUS_MCP_SCOPES: &[Scope] = &[Scope::System, Scope::User, Scope::Project];

pub const TARGETS: &[Target] = &[
    Target {
        name: "claude",
        scopes: &[Scope::Local, Scope::Project, Scope::User],
        status_rules_scopes: &[Scope::System, Scope::User, Scope::Project, Scope::Local],
        status_mcp_scopes: &[Scope::System, Scope::User, Scope::Project, Scope::Local],
        default_scope: Scope::Local,
        forced_mcp_scope: None,
        rules_file: "CLAUDE.md",
        user_dir: ".claude",
        registrar: Registrar::Cli {
            program: "claude",
            style: CliStyle::Claude,
        },
        probe: ProbeKind::ClaudeConfig,
        json: None,
    },
    Target {
        name: "codex",
        scopes: &[Scope::Project, Scope::User],
        status_rules_scopes: &[Scope::User, Scope::Project],
        status_mcp_scopes: CODEX_STATUS_MCP_SCOPES,
        default_scope: Scope::Project,
        forced_mcp_scope: Some(Scope::User),
        rules_file: "AGENTS.md",
        user_dir: ".codex",
        registrar: Registrar::Cli {
            program: "codex",
            style: CliStyle::Codex,
        },
        probe: ProbeKind::CodexList,
        json: None,
    },
    Target {
        name: "gemini",
        scopes: &[Scope::Project, Scope::User],
        status_rules_scopes: &[Scope::User, Scope::Project],
        status_mcp_scopes: &[Scope::System, Scope::User, Scope::Project],
        default_scope: Scope::Project,
        forced_mcp_scope: None,
        rules_file: "GEMINI.md",
        user_dir: ".gemini",
        registrar: Registrar::Cli {
            program: "gemini",
            style: CliStyle::Gemini,
        },
        probe: ProbeKind::Json,
        json: Some(JsonConfig {
            key: "mcpServers",
            project_path: Some(".gemini/settings.json"),
            user_path: Some(".gemini/settings.json"),
        }),
    },
    Target {
        name: "cursor",
        scopes: &[Scope::Project, Scope::User],
        status_rules_scopes: &[Scope::User, Scope::Project],
        status_mcp_scopes: &[Scope::User, Scope::Project],
        default_scope: Scope::Project,
        forced_mcp_scope: None,
        rules_file: ".cursor/rules/ah.mdc",
        user_dir: ".cursor",
        registrar: Registrar::File,
        probe: ProbeKind::Json,
        json: Some(JsonConfig {
            key: "mcpServers",
            project_path: Some(".cursor/mcp.json"),
            user_path: Some(".cursor/mcp.json"),
        }),
    },
    Target {
        name: "copilot",
        scopes: &[Scope::Project],
        status_rules_scopes: &[Scope::Project],
        status_mcp_scopes: &[Scope::User, Scope::Project],
        default_scope: Scope::Project,
        forced_mcp_scope: None,
        rules_file: ".github/copilot-instructions.md",
        user_dir: ".github",
        registrar: Registrar::File,
        probe: ProbeKind::Json,
        json: Some(JsonConfig {
            key: "servers",
            project_path: Some(".vscode/mcp.json"),
            user_path: None,
        }),
    },
    Target {
        name: "opencode",
        scopes: &[Scope::Project, Scope::User],
        status_rules_scopes: &[Scope::User, Scope::Project],
        status_mcp_scopes: &[Scope::System, Scope::User, Scope::Project],
        default_scope: Scope::Project,
        forced_mcp_scope: None,
        rules_file: "AGENTS.md",
        user_dir: ".config/opencode",
        registrar: Registrar::File,
        probe: ProbeKind::OpenCode,
        json: None,
    },
];

pub fn find(name: &str) -> Result<&'static Target, AppError> {
    TARGETS
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| {
            let known = TARGETS
                .iter()
                .map(|target| target.name)
                .collect::<Vec<_>>()
                .join(", ");
            AppError::external(
                "AI_TARGET_UNKNOWN",
                format!("unknown agent target: {name}; known targets: {known}"),
            )
        })
}

impl Target {
    pub fn cli_program(&self) -> Option<&'static str> {
        match self.registrar {
            Registrar::Cli { program, .. } => Some(program),
            Registrar::File => None,
        }
    }

    pub fn cli_style(&self) -> Option<CliStyle> {
        match self.registrar {
            Registrar::Cli { style, .. } => Some(style),
            Registrar::File => None,
        }
    }

    pub fn json_config(&self) -> Result<JsonConfig, AppError> {
        self.json.ok_or_else(|| {
            AppError::external(
                "AI_CONFIG_UNPARSABLE",
                format!("{} has no JSON configuration layout", self.name),
            )
        })
    }

    pub fn supports(&self, scope: Scope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn supports_status_rules(&self, scope: Scope) -> bool {
        self.status_rules_scopes.contains(&scope)
    }

    pub fn supports_status_mcp(&self, scope: Scope) -> bool {
        self.status_mcp_scopes.contains(&scope)
    }

    pub fn require_scope(&self, scope: Scope) -> Result<(), AppError> {
        if self.supports(scope) {
            return Ok(());
        }
        let supported = self
            .scopes
            .iter()
            .map(|scope| scope.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Err(AppError::external(
            "AI_TARGET_SCOPE_UNSUPPORTED",
            format!(
                "{} does not support scope {}; supported scopes: {supported}",
                self.name,
                scope.as_str()
            ),
        ))
    }

    /// The scope the MCP registration actually lands in, which can differ from
    /// the requested scope when the agent has a single global server list.
    pub fn mcp_scope(&self, requested: Scope) -> Scope {
        self.forced_mcp_scope.unwrap_or(requested)
    }

    pub fn rules_path(&self, scope: Scope, project_root: &Path) -> Result<PathBuf, AppError> {
        match scope {
            Scope::Local | Scope::Project => Ok(join_relative(project_root, self.rules_file)),
            Scope::User => Ok(join_relative(
                &join_relative(&home_dir()?, self.user_dir),
                self.rules_file,
            )),
            Scope::System => Err(AppError::external(
                "AI_TARGET_SCOPE_UNSUPPORTED",
                format!("{} has no installable system rules path", self.name),
            )),
        }
    }
}

/// Table paths are written with `/` for readability; joining them component by
/// component keeps the rendered path native on Windows.
fn join_relative(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(base.to_path_buf(), |path, segment| path.join(segment))
}

pub fn home_dir() -> Result<PathBuf, AppError> {
    let layout = ah_paths::Layout::host();
    ah_paths::home_dir(layout, &ah_paths::ProcessEnvironment).map_err(|_| {
        AppError::external(
            "AI_HOME_UNRESOLVED",
            format!(
                "unable to resolve %{}% for agent configuration",
                ah_paths::home_variable(layout)
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{LEGACY_SERVER_NAMES, SERVER_NAME, Scope, find};

    #[test]
    fn unknown_target_lists_known_targets() {
        let error = find("emacs").expect_err("unknown target should fail");
        assert_eq!(error.code(), "AI_TARGET_UNKNOWN");
        assert!(error.detail_message().contains("claude, codex"));
    }

    #[test]
    fn codex_rejects_local_scope_and_forces_user_mcp_scope() {
        let codex = find("codex").expect("codex target exists");
        let error = codex
            .require_scope(Scope::Local)
            .expect_err("codex has no local scope");
        assert_eq!(error.code(), "AI_TARGET_SCOPE_UNSUPPORTED");
        assert!(error.detail_message().contains("project, user"));
        assert_eq!(codex.mcp_scope(Scope::Project), Scope::User);
    }

    #[test]
    fn copilot_is_project_only_and_uses_its_own_server_key() {
        let copilot = find("copilot").expect("copilot target exists");
        assert!(copilot.supports(Scope::Project));
        let error = copilot
            .require_scope(Scope::User)
            .expect_err("copilot has no user scope");
        assert_eq!(error.code(), "AI_TARGET_SCOPE_UNSUPPORTED");
        assert_eq!(copilot.json_config().expect("json layout").key, "servers");
    }

    #[test]
    fn opencode_supports_project_and_user_configuration() {
        let opencode = find("opencode").expect("opencode target exists");
        assert!(opencode.supports(Scope::Project));
        assert!(opencode.supports(Scope::User));
        assert_eq!(opencode.rules_file, "AGENTS.md");
        assert!(opencode.cli_program().is_none());
    }

    #[test]
    fn table_paths_render_with_native_separators() {
        let cursor = find("cursor").expect("cursor target exists");
        let path = cursor
            .rules_path(Scope::Project, Path::new("/project"))
            .expect("rules path resolves");
        assert_eq!(
            path,
            Path::new("/project")
                .join(".cursor")
                .join("rules")
                .join("ah.mdc")
        );
        if cfg!(windows) {
            let tail = path
                .strip_prefix(Path::new("/project"))
                .expect("the resolved path starts at the project root");
            assert!(
                !tail.display().to_string().contains('/'),
                "a Windows path must not carry forward slashes from the table"
            );
        }
    }

    #[test]
    fn json_paths_follow_the_scope() {
        let cursor = find("cursor").expect("cursor target exists");
        let config = cursor.json_config().expect("json layout");
        let root = Path::new("/project");
        assert_eq!(
            config
                .path(Scope::Project, root)
                .expect("project path resolves"),
            root.join(".cursor/mcp.json")
        );
        assert!(
            config
                .path(Scope::User, root)
                .expect("user path resolves")
                .ends_with(".cursor/mcp.json")
        );
    }

    #[test]
    fn the_legacy_server_name_is_not_the_current_one() {
        assert_eq!(SERVER_NAME, "aihelper");
        assert!(
            LEGACY_SERVER_NAMES.contains(&"ah"),
            "installations made under the old name must stay discoverable"
        );
        assert!(
            !LEGACY_SERVER_NAMES.contains(&SERVER_NAME),
            "the current name must never be treated as legacy"
        );
    }

    #[test]
    fn claude_keeps_the_requested_scope() {
        let claude = find("claude").expect("claude target exists");
        claude
            .require_scope(Scope::Local)
            .expect("claude supports local scope");
        assert_eq!(claude.mcp_scope(Scope::Project), Scope::Project);
    }
}
