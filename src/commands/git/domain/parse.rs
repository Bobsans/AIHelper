//! Reading git's output formats: porcelain blame, name-status, numstat and the
//! remote list.
//!
//! Each is a separate ad-hoc format, and none of them is ours.

use super::*;

pub(super) fn changed_entry(entry: StatusEntry) -> ChangedEntry {
    ChangedEntry {
        status: entry.status,
        path: normalize_slashes(&entry.path),
        old_path: entry.old_path.map(|path| normalize_slashes(&path)),
    }
}

pub(super) fn blame_header_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([0-9a-f^]{7,40})\s+\d+\s+(\d+)(?:\s+\d+)?$").unwrap())
}

pub(super) fn parse_line_porcelain(raw: &str) -> Result<Vec<BlameEntry>, AppError> {
    let header_re = blame_header_regex();

    let mut entries = Vec::new();
    let mut lines = raw.lines().peekable();

    while let Some(line) = lines.next() {
        let Some(captures) = header_re.captures(line) else {
            continue;
        };

        let commit = captures[1].to_owned();
        let final_line = captures[2].parse::<usize>().unwrap_or(0);

        let mut author = String::new();
        let mut author_mail = String::new();
        let mut author_time = None;
        let mut summary = String::new();
        let mut text = String::new();

        for metadata_line in lines.by_ref() {
            if let Some(value) = metadata_line.strip_prefix('\t') {
                text = value.to_owned();
                break;
            }
            if let Some(value) = metadata_line.strip_prefix("author ") {
                author = value.to_owned();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("author-mail ") {
                author_mail = value.trim_matches(['<', '>']).to_owned();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("author-time ") {
                author_time = value.parse::<i64>().ok();
                continue;
            }
            if let Some(value) = metadata_line.strip_prefix("summary ") {
                summary = value.to_owned();
            }
        }

        entries.push(BlameEntry {
            line: final_line,
            commit,
            author,
            author_mail,
            author_time,
            summary,
            text,
        });
    }

    Ok(entries)
}

pub(super) fn parse_name_status(raw: &str) -> Vec<CommitFile> {
    raw.lines()
        .filter_map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            let status = parts.first()?.to_string();
            if status.starts_with('R') || status.starts_with('C') {
                let old_path = parts.get(1).map(|value| normalize_slashes(value));
                let path = parts.get(2).map(|value| normalize_slashes(value))?;
                Some(CommitFile {
                    status: Some(status),
                    path,
                    old_path,
                    additions: None,
                    deletions: None,
                })
            } else {
                let path = parts.get(1).map(|value| normalize_slashes(value))?;
                Some(CommitFile {
                    status: Some(status),
                    path,
                    old_path: None,
                    additions: None,
                    deletions: None,
                })
            }
        })
        .collect()
}

pub(super) fn parse_numstat(raw: &str) -> Vec<CommitFile> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let additions = parse_optional_usize(parts.next()?);
            let deletions = parse_optional_usize(parts.next()?);
            let path = normalize_slashes(parts.next()?);
            Some(CommitFile {
                status: None,
                path,
                old_path: None,
                additions,
                deletions,
            })
        })
        .collect()
}

pub(super) fn parse_remotes(raw: &str) -> Vec<RemoteEntry> {
    let mut remotes: Vec<RemoteEntry> = Vec::new();
    for line in raw.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        let Some(kind) = parts.next() else { continue };
        let entry_index = remotes
            .iter()
            .position(|entry| entry.name == name)
            .unwrap_or_else(|| {
                remotes.push(RemoteEntry {
                    name: name.to_owned(),
                    fetch_url: None,
                    push_url: None,
                    provider: "unknown".to_owned(),
                });
                remotes.len() - 1
            });
        let entry = &mut remotes[entry_index];
        match kind {
            "(fetch)" => entry.fetch_url = Some(url.to_owned()),
            "(push)" => entry.push_url = Some(url.to_owned()),
            _ => {}
        }
        entry.provider = detect_provider(entry.fetch_url.as_deref().or(entry.push_url.as_deref()));
    }
    remotes
}

pub(super) fn parse_optional_usize(raw: &str) -> Option<usize> {
    raw.parse::<usize>().ok()
}

pub(super) fn optional_trimmed(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

pub(super) fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
}

pub(super) fn detect_provider(url: Option<&str>) -> String {
    let Some(url) = url else {
        return "unknown".to_owned();
    };
    let lower = url.to_ascii_lowercase();
    if lower.contains("github.com") {
        "github".to_owned()
    } else if lower.contains("gitlab.com") {
        "gitlab".to_owned()
    } else if lower.contains("bitbucket.org") {
        "bitbucket".to_owned()
    } else {
        "unknown".to_owned()
    }
}

pub(super) fn sum_optional(values: impl Iterator<Item = Option<usize>>) -> Option<usize> {
    let mut saw_value = false;
    let mut total = 0usize;
    for value in values.flatten() {
        saw_value = true;
        total += value;
    }
    if saw_value { Some(total) } else { None }
}

pub(super) fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

pub(super) fn normalize_path(path: &str) -> String {
    normalize_slashes(path)
}

pub(super) fn is_no_commit_error(error: &AppError) -> bool {
    match error {
        AppError::CommandFailed { stderr, .. } => {
            stderr.contains("no such ref: HEAD")
                || stderr.contains("has no commits yet")
                || stderr.contains("no commits yet")
        }
        _ => false,
    }
}
