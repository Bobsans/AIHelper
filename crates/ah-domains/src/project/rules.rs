#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileGroup {
    Package,
    Lock,
    Ci,
    Docs,
    Changelog,
    Deploy,
    Infra,
    Config,
    Quality,
    Security,
}

#[derive(Debug, Clone, Copy)]
pub struct FileRuleDetection {
    pub group: FileGroup,
    pub kind: &'static str,
    pub ecosystem: Option<&'static str>,
    pub tool: Option<&'static str>,
    pub role: Option<&'static str>,
}

/// When a rule applies.
///
/// Every comparison is against the lowercased file name or the lowercased path
/// relative to the project root, so a rule never has to say which.
#[derive(Debug, Clone, Copy)]
enum When {
    Name(&'static str),
    NameStartsWith(&'static str),
    NameEndsWith(&'static str),
    Path(&'static str),
    PathStartsWith(&'static str),
    PathContains(&'static str),
    Any(&'static [When]),
    All(&'static [When]),
}

impl When {
    fn matches(&self, path: &str, name: &str) -> bool {
        match self {
            Self::Name(value) => name == *value,
            Self::NameStartsWith(value) => name.starts_with(value),
            Self::NameEndsWith(value) => name.ends_with(value),
            Self::Path(value) => path == *value,
            Self::PathStartsWith(value) => path.starts_with(value),
            Self::PathContains(value) => path.contains(value),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.matches(path, name)),
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.matches(path, name)),
        }
    }
}

/// One classification: what has to be true, and what that means.
#[derive(Debug, Clone, Copy)]
struct FileRule {
    when: When,
    detection: FileRuleDetection,
}

/// A table entry.
///
/// This is a function rather than a struct literal only so that rustfmt keeps
/// one rule on one line: 121 four-line literals read as code rather than as the
/// data they are.
const fn rule(when: When, detection: FileRuleDetection) -> FileRule {
    FileRule { when, detection }
}

mod fixtures;
mod table;

use table::RULES;

#[cfg(any(test, feature = "fixtures"))]
pub use fixtures::PROJECT_RULE_FIXTURES;

/// Every rule whose condition holds, in table order.
pub fn classify_file(rel: &str, name: &str) -> Vec<FileRuleDetection> {
    let path = rel.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    RULES
        .iter()
        .filter(|rule| rule.when.matches(&path, &name))
        .map(|rule| rule.detection)
        .collect()
}

const fn package(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Package,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn lock(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Lock,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn ci(kind: &'static str, tool: &'static str) -> FileRuleDetection {
    grouped(FileGroup::Ci, kind, None, Some(tool), Some("ci"))
}

const fn deploy(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
    role: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Deploy,
        kind,
        Some(ecosystem),
        Some(tool),
        Some(role),
    )
}

const fn config(
    kind: &'static str,
    ecosystem: &'static str,
    tool: &'static str,
) -> FileRuleDetection {
    grouped(
        FileGroup::Config,
        kind,
        Some(ecosystem),
        Some(tool),
        Some("source"),
    )
}

const fn grouped(
    group: FileGroup,
    kind: &'static str,
    ecosystem: Option<&'static str>,
    tool: Option<&'static str>,
    role: Option<&'static str>,
) -> FileRuleDetection {
    FileRuleDetection {
        group,
        kind,
        ecosystem,
        tool,
        role,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule nothing can reach is dead weight that still reads as coverage.
    /// The snapshot corpus is built from these literals, so every rule should
    /// fire for at least one of its paths.
    #[test]
    fn every_rule_fires_for_the_snapshot_corpus() {
        let mut unreached = Vec::new();
        for (index, rule) in RULES.iter().enumerate() {
            let fired = PROJECT_RULE_FIXTURES.iter().any(|(rel, name)| {
                rule.when
                    .matches(&rel.to_ascii_lowercase(), &name.to_ascii_lowercase())
            });
            if !fired {
                unreached.push(format!("{index}:{}", rule.detection.kind));
            }
        }

        assert!(
            unreached.is_empty(),
            "rules no fixture reaches: {unreached:?}"
        );
    }
}
