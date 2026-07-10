use crate::error::AppError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusEntry {
    pub(crate) index_status: u8,
    pub(crate) worktree_status: u8,
    pub(crate) status: String,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatusCounts {
    pub(crate) staged: usize,
    pub(crate) unstaged: usize,
    pub(crate) untracked: usize,
}

pub(crate) fn parse_porcelain_v1_z(raw: &[u8]) -> Result<Vec<StatusEntry>, AppError> {
    let mut cursor = 0usize;
    let mut entries = Vec::new();

    while cursor < raw.len() {
        let record = next_nul_field(raw, &mut cursor)?;
        if record.len() < 3 || record[2] != b' ' {
            return Err(invalid_porcelain("status entry is shorter than 'XY path'"));
        }

        let index_status = record[0];
        let worktree_status = record[1];
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        let old_path =
            if matches!(index_status, b'R' | b'C') || matches!(worktree_status, b'R' | b'C') {
                Some(String::from_utf8_lossy(next_nul_field(raw, &mut cursor)?).into_owned())
            } else {
                None
            };

        entries.push(StatusEntry {
            index_status,
            worktree_status,
            status: String::from_utf8_lossy(&record[..2]).trim().to_owned(),
            path,
            old_path,
        });
    }

    Ok(entries)
}

pub(crate) fn count_statuses(entries: &[StatusEntry]) -> StatusCounts {
    let mut counts = StatusCounts::default();
    for entry in entries {
        if entry.index_status == b'?' && entry.worktree_status == b'?' {
            counts.untracked += 1;
            continue;
        }
        if entry.index_status != b' ' {
            counts.staged += 1;
        }
        if entry.worktree_status != b' ' {
            counts.unstaged += 1;
        }
    }
    counts
}

fn next_nul_field<'a>(raw: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], AppError> {
    let remaining = &raw[*cursor..];
    let Some(end) = remaining.iter().position(|byte| *byte == 0) else {
        return Err(invalid_porcelain(
            "status output is missing a NUL terminator",
        ));
    };
    *cursor += end + 1;
    Ok(&remaining[..end])
}

fn invalid_porcelain(reason: &str) -> AppError {
    AppError::external(
        "GIT_RESPONSE_INVALID",
        format!("failed to parse git status --porcelain=v1 -z: {reason}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literal_arrow_newline_and_rename() {
        let raw = b" M file -> name\0R  new name\0old name\0?? line\nbreak\0";
        let entries = parse_porcelain_v1_z(raw).expect("porcelain should parse");

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "file -> name");
        assert_eq!(entries[0].old_path, None);
        assert_eq!(entries[1].path, "new name");
        assert_eq!(entries[1].old_path.as_deref(), Some("old name"));
        assert_eq!(entries[2].path, "line\nbreak");
        assert_eq!(
            count_statuses(&entries),
            StatusCounts {
                staged: 1,
                unstaged: 1,
                untracked: 1,
            }
        );
    }

    #[test]
    fn rejects_non_terminated_output() {
        let error = parse_porcelain_v1_z(b" M file").expect_err("missing NUL must fail");
        assert!(error.detail_message().contains("missing a NUL terminator"));
    }
}
