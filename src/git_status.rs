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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct StatusSnapshot {
    pub(crate) branch: Option<String>,
    pub(crate) upstream: Option<String>,
    pub(crate) ahead: Option<usize>,
    pub(crate) behind: Option<usize>,
    pub(crate) entries: Vec<StatusEntry>,
}

pub(crate) fn parse_porcelain_v2_branch_z(raw: &[u8]) -> Result<StatusSnapshot, AppError> {
    let mut cursor = 0usize;
    let mut snapshot = StatusSnapshot::default();

    while cursor < raw.len() {
        let record = next_nul_field(raw, &mut cursor)?;
        if let Some(header) = record.strip_prefix(b"# ") {
            parse_branch_header(header, &mut snapshot)?;
            continue;
        }
        if let Some(path) = record.strip_prefix(b"? ") {
            snapshot.entries.push(status_entry(b'?', b'?', path, None));
            continue;
        }
        if record.starts_with(b"! ") {
            continue;
        }

        match record.first().copied() {
            Some(b'1') => snapshot.entries.push(parse_v2_entry(record, 9, None)?),
            Some(b'2') => {
                let old_path = next_nul_field(raw, &mut cursor)?;
                snapshot
                    .entries
                    .push(parse_v2_entry(record, 10, Some(old_path))?);
            }
            Some(b'u') => snapshot.entries.push(parse_v2_entry(record, 11, None)?),
            _ => return Err(invalid_porcelain_v2("unknown status record type")),
        }
    }

    Ok(snapshot)
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

fn parse_branch_header(header: &[u8], snapshot: &mut StatusSnapshot) -> Result<(), AppError> {
    let Some((key, value)) = split_once_byte(header, b' ') else {
        return Err(invalid_porcelain_v2("branch header is missing a value"));
    };
    match key {
        b"branch.head" if value != b"(detached)" => {
            snapshot.branch = Some(String::from_utf8_lossy(value).into_owned());
        }
        b"branch.upstream" => {
            snapshot.upstream = Some(String::from_utf8_lossy(value).into_owned());
        }
        b"branch.ab" => {
            let mut counts = value.split(|byte| *byte == b' ');
            snapshot.ahead = parse_prefixed_count(counts.next(), b'+')?;
            snapshot.behind = parse_prefixed_count(counts.next(), b'-')?;
        }
        _ => {}
    }
    Ok(())
}

fn parse_v2_entry(
    record: &[u8],
    field_count: usize,
    old_path: Option<&[u8]>,
) -> Result<StatusEntry, AppError> {
    let fields = record
        .splitn(field_count, |byte| *byte == b' ')
        .collect::<Vec<_>>();
    if fields.len() != field_count || fields[1].len() != 2 {
        return Err(invalid_porcelain_v2("status entry has invalid fields"));
    }
    let index_status = normalize_v2_status(fields[1][0]);
    let worktree_status = normalize_v2_status(fields[1][1]);
    Ok(status_entry(
        index_status,
        worktree_status,
        fields[field_count - 1],
        old_path,
    ))
}

fn status_entry(
    index_status: u8,
    worktree_status: u8,
    path: &[u8],
    old_path: Option<&[u8]>,
) -> StatusEntry {
    StatusEntry {
        index_status,
        worktree_status,
        status: String::from_utf8_lossy(&[index_status, worktree_status])
            .trim()
            .to_owned(),
        path: String::from_utf8_lossy(path).into_owned(),
        old_path: old_path.map(|path| String::from_utf8_lossy(path).into_owned()),
    }
}

fn normalize_v2_status(status: u8) -> u8 {
    if status == b'.' { b' ' } else { status }
}

fn parse_prefixed_count(value: Option<&[u8]>, prefix: u8) -> Result<Option<usize>, AppError> {
    let Some(value) = value else {
        return Err(invalid_porcelain_v2("branch.ab is missing a count"));
    };
    let Some(value) = value.strip_prefix(&[prefix]) else {
        return Err(invalid_porcelain_v2(
            "branch.ab count has an invalid prefix",
        ));
    };
    String::from_utf8_lossy(value)
        .parse::<usize>()
        .map(Some)
        .map_err(|_| invalid_porcelain_v2("branch.ab count is not numeric"))
}

fn split_once_byte(value: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let index = value.iter().position(|byte| *byte == separator)?;
    Some((&value[..index], &value[index + 1..]))
}

fn invalid_porcelain(reason: &str) -> AppError {
    AppError::external(
        "GIT_RESPONSE_INVALID",
        format!("failed to parse git status --porcelain=v1 -z: {reason}"),
    )
}

fn invalid_porcelain_v2(reason: &str) -> AppError {
    AppError::external(
        "GIT_RESPONSE_INVALID",
        format!("failed to parse git status --porcelain=v2 --branch -z: {reason}"),
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

    #[test]
    fn parses_v2_branch_counts_entries_and_rename() {
        let raw = b"# branch.oid abc123\x00# branch.head feature/test\x00# branch.upstream origin/feature/test\x00# branch.ab +2 -3\x001 .M N... 100644 100644 100644 a b file -> name\x002 R. N... 100644 100644 100644 a b R100 renamed file\x00old file\x00? line\nbreak\x00";
        let snapshot = parse_porcelain_v2_branch_z(raw).expect("porcelain v2 should parse");

        assert_eq!(snapshot.branch.as_deref(), Some("feature/test"));
        assert_eq!(snapshot.upstream.as_deref(), Some("origin/feature/test"));
        assert_eq!(snapshot.ahead, Some(2));
        assert_eq!(snapshot.behind, Some(3));
        assert_eq!(snapshot.entries.len(), 3);
        assert_eq!(snapshot.entries[0].status, "M");
        assert_eq!(snapshot.entries[0].path, "file -> name");
        assert_eq!(snapshot.entries[1].status, "R");
        assert_eq!(snapshot.entries[1].path, "renamed file");
        assert_eq!(snapshot.entries[1].old_path.as_deref(), Some("old file"));
        assert_eq!(snapshot.entries[2].path, "line\nbreak");
        assert_eq!(
            count_statuses(&snapshot.entries),
            StatusCounts {
                staged: 1,
                unstaged: 1,
                untracked: 1,
            }
        );
    }

    #[test]
    fn parses_v2_detached_and_conflicted_status() {
        let raw =
            b"# branch.head (detached)\0u UU N... 100644 100644 100644 100644 a b c conflict.txt\0";
        let snapshot = parse_porcelain_v2_branch_z(raw).expect("porcelain v2 should parse");
        assert_eq!(snapshot.branch, None);
        assert_eq!(snapshot.entries[0].status, "UU");
        assert_eq!(
            count_statuses(&snapshot.entries),
            StatusCounts {
                staged: 1,
                unstaged: 1,
                untracked: 0,
            }
        );
    }

    #[test]
    fn rejects_malformed_v2_records() {
        let error = parse_porcelain_v2_branch_z(b"# branch.ab +x -0\0")
            .expect_err("non-numeric count should fail");
        assert_eq!(error.code(), "GIT_RESPONSE_INVALID");
    }
}
