use std::path::Path;

use serde::Serialize;

use crate::error::AppError;

pub const BEGIN_MARKER: &str = "<!-- ah:begin (managed by `ah ai install`) -->";
pub const END_MARKER: &str = "<!-- ah:end -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockAction {
    Installed,
    Updated,
    Unchanged,
    Removed,
    NotPresent,
    Skipped,
}

impl BlockAction {
    pub fn changed(self) -> bool {
        matches!(self, Self::Installed | Self::Updated | Self::Removed)
    }
}

fn malformed(path_hint: &str) -> AppError {
    AppError::external(
        "AI_RULES_BLOCK_MALFORMED",
        format!(
            "{path_hint} contains an unbalanced AIHelper block; \
             restore the `{BEGIN_MARKER}` / `{END_MARKER}` pair or remove it by hand"
        ),
    )
}

fn locate(contents: &str, path_hint: &str) -> Result<Option<(usize, usize)>, AppError> {
    let begin = contents.find(BEGIN_MARKER);
    let end = contents.find(END_MARKER);
    match (begin, end) {
        (None, None) => Ok(None),
        (Some(begin), Some(end)) if end > begin => Ok(Some((begin, end + END_MARKER.len()))),
        _ => Err(malformed(path_hint)),
    }
}

/// Insert or replace the managed block, leaving every other line untouched.
pub fn upsert(
    existing: Option<&str>,
    block: &str,
    path_hint: &str,
) -> Result<(String, BlockAction), AppError> {
    let contents = existing.unwrap_or_default();
    let Some((begin, end)) = locate(contents, path_hint)? else {
        let mut updated = String::from(contents);
        if !updated.is_empty() {
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push('\n');
        }
        updated.push_str(block);
        updated.push('\n');
        return Ok((updated, BlockAction::Installed));
    };
    if &contents[begin..end] == block {
        return Ok((contents.to_owned(), BlockAction::Unchanged));
    }
    let mut updated = String::with_capacity(contents.len() + block.len());
    updated.push_str(&contents[..begin]);
    updated.push_str(block);
    updated.push_str(&contents[end..]);
    Ok((updated, BlockAction::Updated))
}

/// Remove exactly the managed block and the blank line that separated it.
pub fn strip(contents: &str, path_hint: &str) -> Result<(String, BlockAction), AppError> {
    let Some((begin, end)) = locate(contents, path_hint)? else {
        return Ok((contents.to_owned(), BlockAction::NotPresent));
    };
    let mut updated = String::with_capacity(contents.len());
    updated.push_str(contents[..begin].trim_end_matches(['\n', '\r']));
    let tail = contents[end..].trim_start_matches(['\n', '\r']);
    if !updated.is_empty() && !tail.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(tail);
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    Ok((updated, BlockAction::Removed))
}

pub fn read(path: &Path) -> Result<Option<String>, AppError> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(AppError::file_read(path.to_path_buf(), source)),
    }
}

pub fn write(path: &Path, contents: &str) -> Result<(), AppError> {
    crate::persistence::atomic_write(path, contents.as_bytes())
}

pub fn delete(path: &Path) -> Result<(), AppError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(AppError::file_write(path.to_path_buf(), source)),
    }
}

/// The block points at the live manual instead of copying it, so it cannot go
/// stale when plugins change. Only the domain list is materialized.
pub fn render_block(domains: &[String]) -> String {
    let domain_list = if domains.is_empty() {
        "(none enabled)".to_owned()
    } else {
        domains.join(", ")
    };
    // Assembled line by line: a wrapped list item has to keep its two-space
    // continuation indent, which a `\`-continued string literal would strip.
    [
        BEGIN_MARKER,
        "## AIHelper (`ah`)",
        "",
        "AIHelper is available as MCP tools (`ah.*`) and as the `ah` CLI. Both expose",
        "the same typed command catalog.",
        "",
        "The authoritative manual is `ah ai info --json`, narrowable with",
        "`ah ai info --domain <domain>`. Read it instead of guessing flags; this block",
        "is only a pointer and never a copy.",
        "",
        &format!("Available domains: {domain_list}."),
        "",
        "Guidance:",
        "",
        "- Prefer `ah` over ad-hoc shell for file reads, file and text search, git",
        "  context, project detection, and checked command execution (`ah run check`).",
        "- `ah` output is deterministic and supports `--json` for machine reading.",
        "- Secrets are exposed only as redacted identifiers through `secrets.list`.",
        "  Their values are never readable by an agent and must not be requested,",
        "  echoed, or written into commands.",
        END_MARKER,
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{BEGIN_MARKER, BlockAction, END_MARKER, render_block, strip, upsert};

    fn block() -> String {
        render_block(&["file".to_owned(), "git".to_owned()])
    }

    #[test]
    fn insert_into_missing_file_produces_only_the_block() {
        let (contents, action) = upsert(None, &block(), "CLAUDE.md").expect("upsert should work");
        assert_eq!(action, BlockAction::Installed);
        assert!(contents.starts_with(BEGIN_MARKER));
        assert!(contents.trim_end().ends_with(END_MARKER));
    }

    #[test]
    fn insert_preserves_existing_user_content() {
        let existing = "# My rules\n\nAlways be nice.\n";
        let (contents, action) = upsert(Some(existing), &block(), "CLAUDE.md").expect("upsert");
        assert_eq!(action, BlockAction::Installed);
        assert!(contents.starts_with("# My rules\n\nAlways be nice.\n\n"));
        assert!(contents.contains(BEGIN_MARKER));
    }

    #[test]
    fn repeated_install_is_unchanged_and_a_changed_block_is_replaced_in_place() {
        let existing = format!("keep me\n\n{}\n\ntrailing\n", block());
        let (same, action) = upsert(Some(&existing), &block(), "CLAUDE.md").expect("upsert");
        assert_eq!(action, BlockAction::Unchanged);
        assert_eq!(same, existing);

        let updated_block = render_block(&["file".to_owned()]);
        let (updated, action) =
            upsert(Some(&existing), &updated_block, "CLAUDE.md").expect("upsert");
        assert_eq!(action, BlockAction::Updated);
        assert!(updated.starts_with("keep me\n\n"));
        assert!(updated.trim_end().ends_with("trailing"));
        assert!(updated.contains("Available domains: file."));
        assert!(!updated.contains("Available domains: file, git."));
    }

    #[test]
    fn strip_removes_only_the_block() {
        let existing = format!("keep me\n\n{}\n\ntrailing\n", block());
        let (contents, action) = strip(&existing, "CLAUDE.md").expect("strip should work");
        assert_eq!(action, BlockAction::Removed);
        assert_eq!(contents, "keep me\n\ntrailing\n");
    }

    #[test]
    fn strip_of_a_block_only_file_leaves_nothing() {
        let existing = format!("{}\n", block());
        let (contents, action) = strip(&existing, "AGENTS.md").expect("strip should work");
        assert_eq!(action, BlockAction::Removed);
        assert!(contents.is_empty());
    }

    #[test]
    fn strip_without_a_block_reports_not_present() {
        let (contents, action) = strip("nothing here\n", "AGENTS.md").expect("strip should work");
        assert_eq!(action, BlockAction::NotPresent);
        assert_eq!(contents, "nothing here\n");
    }

    #[test]
    fn an_unbalanced_block_is_rejected_rather_than_guessed() {
        let broken = format!("{BEGIN_MARKER}\nhalf a block\n");
        let error = upsert(Some(&broken), &block(), "CLAUDE.md").expect_err("must fail");
        assert_eq!(error.code(), "AI_RULES_BLOCK_MALFORMED");
        let error = strip(&broken, "CLAUDE.md").expect_err("must fail");
        assert_eq!(error.code(), "AI_RULES_BLOCK_MALFORMED");
    }

    #[test]
    fn a_lone_end_marker_is_also_rejected() {
        let broken = format!("text\n{END_MARKER}\n");
        let error = upsert(Some(&broken), &block(), "CLAUDE.md").expect_err("must fail");
        assert_eq!(error.code(), "AI_RULES_BLOCK_MALFORMED");
    }
}
