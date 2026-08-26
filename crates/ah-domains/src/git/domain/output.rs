//! What each `ah git` subcommand reports.

use super::*;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangedEntry {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitChangedOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub changed_count: usize,
    pub truncated: bool,
    pub entries: Vec<ChangedEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitStatusOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    pub clean: bool,
    pub staged_count: usize,
    pub unstaged_count: usize,
    pub untracked_count: usize,
    pub changed_count: usize,
    pub latest_commit: Option<CommitSummary>,
    pub latest_tag: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitSummary {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) subject: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitInfoOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub reference: String,
    pub commit: Option<CommitInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitInfo {
    pub(crate) hash: String,
    pub(crate) short_hash: String,
    pub(crate) author: GitPerson,
    pub(crate) author_date: Option<String>,
    pub(crate) committer: GitPerson,
    pub(crate) committer_date: Option<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
    pub(crate) file_count: usize,
    pub(crate) additions: Option<usize>,
    pub(crate) deletions: Option<usize>,
    pub(crate) files: Vec<CommitFile>,
    pub(crate) truncated: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitPerson {
    pub(crate) name: String,
    pub(crate) email: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitFile {
    pub(crate) status: Option<String>,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) additions: Option<usize>,
    pub(crate) deletions: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagEntry {
    pub(crate) name: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitTagsOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub latest: bool,
    pub tag_count: usize,
    pub truncated: bool,
    pub tags: Vec<TagEntry>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteEntry {
    pub(crate) name: String,
    pub(crate) fetch_url: Option<String>,
    pub(crate) push_url: Option<String>,
    pub(crate) provider: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitRemotesOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub remote_count: usize,
    pub remotes: Vec<RemoteEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitDiffOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub path_filter: Option<String>,
    pub line_count: usize,
    pub truncated: bool,
    pub diff: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BlameEntry {
    #[schemars(range(min = 1))]
    pub(crate) line: usize,
    pub(crate) commit: String,
    pub(crate) author: String,
    pub(crate) author_mail: String,
    pub(crate) author_time: Option<i64>,
    pub(crate) summary: String,
    pub(crate) text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitBlameOutput {
    pub command: &'static str,
    pub path: String,
    #[schemars(range(min = 1))]
    pub line_filter: Option<usize>,
    pub entry_count: usize,
    pub truncated: bool,
    pub entries: Vec<BlameEntry>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GitTagCreateOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub tag: String,
    pub reference: String,
    pub annotated: bool,
    pub target_commit: Option<CommitSummary>,
}

#[derive(Debug)]
pub(crate) enum GitResult {
    Status(GitStatusOutput),
    Tags(GitTagsOutput),
    Remotes(GitRemotesOutput),
    Changed(GitChangedOutput),
    Diff(GitDiffOutput),
    Blame {
        payload: GitBlameOutput,
        in_git_repo: bool,
    },
    CommitInfo(CommitInfoOutput),
    TagCreate(GitTagCreateOutput),
}

pub(super) fn empty_status_result() -> GitResult {
    GitResult::Status(GitStatusOutput {
        command: "git.status",
        in_git_repo: false,
        branch: None,
        upstream: None,
        ahead: None,
        behind: None,
        clean: true,
        staged_count: 0,
        unstaged_count: 0,
        untracked_count: 0,
        changed_count: 0,
        latest_commit: None,
        latest_tag: None,
    })
}
