//! What `project detect`, `project commands` and `project version` report.

use super::*;

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetectedFile {
    pub(crate) kind: String,
    pub(crate) path: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SuggestedCommand {
    pub(crate) kind: String,
    pub(crate) command: Vec<String>,
    pub(crate) confidence: String,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectFileGroups {
    pub(crate) packages: Vec<DetectedFile>,
    pub(crate) locks: Vec<DetectedFile>,
    pub(crate) ci: Vec<DetectedFile>,
    pub(crate) docs: Vec<DetectedFile>,
    pub(crate) changelogs: Vec<DetectedFile>,
    pub(crate) deploy: Vec<DetectedFile>,
    pub(crate) infra: Vec<DetectedFile>,
    pub(crate) config: Vec<DetectedFile>,
    pub(crate) quality: Vec<DetectedFile>,
    pub(crate) security: Vec<DetectedFile>,
}

#[derive(Debug, Clone)]
pub(super) struct ProjectSnapshot {
    pub(super) root: String,
    pub(super) ecosystems: Vec<String>,
    pub(super) tools: Vec<String>,
    pub(super) roles: Vec<String>,
    pub(super) files: ProjectFileGroups,
    pub(super) versions: Vec<ProjectVersionEntry>,
    pub(super) commands: Vec<SuggestedCommand>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectVersionEntry {
    pub(crate) kind: String,
    pub(crate) path: String,
    pub(crate) name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) confidence: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectDetectOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) ecosystems: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) roles: Vec<String>,
    pub(crate) files: ProjectFileGroups,
    pub(crate) versions: Vec<ProjectVersionEntry>,
    pub(crate) commands: Vec<SuggestedCommand>,
    #[serde(rename = "package_files")]
    pub(crate) package_files: Vec<DetectedFile>,
    #[serde(rename = "ci_files")]
    pub(crate) ci_files: Vec<DetectedFile>,
    #[serde(rename = "docs_files")]
    pub(crate) docs_files: Vec<DetectedFile>,
    #[serde(rename = "changelog_files")]
    pub(crate) changelog_files: Vec<DetectedFile>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectCommandsOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) ecosystems: Vec<String>,
    pub(crate) tools: Vec<String>,
    pub(crate) roles: Vec<String>,
    pub(crate) commands: Vec<SuggestedCommand>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectVersionOutput {
    pub(crate) command: &'static str,
    pub(crate) root: String,
    pub(crate) version_count: usize,
    pub(crate) truncated: bool,
    pub(crate) versions: Vec<ProjectVersionEntry>,
}
