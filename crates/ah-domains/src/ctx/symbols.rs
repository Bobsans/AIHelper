use std::{path::Path, sync::LazyLock};

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Symbol {
    #[schemars(range(min = 1))]
    pub line: usize,
    pub kind: String,
    pub name: String,
}

/// Where a matched symbol's kind comes from.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// The pattern only ever yields this kind.
    Fixed(&'static str),
    /// The kind is whatever the pattern captured, so one row covers
    /// `struct`/`enum`/`trait` rather than three.
    Captured(usize),
}

/// How a matched symbol's name is assembled.
#[derive(Debug, Clone, Copy)]
enum Name {
    /// One capture group is the name.
    Capture(usize),
    /// `head.tail` when the second group matched, otherwise `head` - the shape
    /// Terraform blocks have, where `resource "aws_s3_bucket" "logs"` is one
    /// name and `module "network"` is another.
    Dotted { head: usize, tail: usize },
    /// The first group when it matched, otherwise the fallback - a Dockerfile
    /// stage is named by its `AS` alias when it has one and by its image
    /// otherwise.
    Preferred { first: usize, fallback: usize },
}

/// One line pattern: what to match, what kind it yields, how the name is built.
#[derive(Debug, Clone, Copy)]
struct Pattern {
    regex: &'static str,
    kind: Kind,
    name: Name,
}

/// How one language is recognised and what its lines mean.
///
/// Pattern order is significant, and stating it as data is the point: the first
/// pattern to match a line wins, so the order here reproduces the order the
/// hand-written extractors tested their regexes in. Reordering rows changes
/// user-visible output.
#[derive(Debug, Clone, Copy)]
struct LanguageSpec {
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    filename_prefixes: &'static [&'static str],
    patterns: &'static [Pattern],
}

mod fixtures;
mod table;

use table::LANGUAGES;

#[cfg(any(test, feature = "fixtures"))]
pub use fixtures::SYMBOL_FIXTURES;

/// Compiled once for the process. Compiling per call would undo the caching the
/// per-regex `OnceLock` accessors used to provide, and extraction runs once per
/// file across a whole tree.
static COMPILED: LazyLock<Vec<Vec<Regex>>> = LazyLock::new(|| {
    LANGUAGES
        .iter()
        .map(|spec| {
            spec.patterns
                .iter()
                .map(|pattern| Regex::new(pattern.regex).expect("valid ctx symbol regex"))
                .collect()
        })
        .collect()
});

pub fn extract_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    if let Some(index) = language_for(&extension, &file_name) {
        return extract_with(&COMPILED[index], LANGUAGES[index].patterns, content);
    }
    if is_markdown(&extension) {
        return extract_markdown_symbols(content);
    }
    extract_generic_symbols(content)
}

/// Extension first, then whole file name, then file-name prefix - the order the
/// hand-written `match` tested them in.
fn language_for(extension: &str, file_name: &str) -> Option<usize> {
    LANGUAGES
        .iter()
        .position(|spec| spec.extensions.contains(&extension))
        .or_else(|| {
            LANGUAGES
                .iter()
                .position(|spec| spec.filenames.contains(&file_name))
        })
        .or_else(|| {
            LANGUAGES.iter().position(|spec| {
                spec.filename_prefixes
                    .iter()
                    .any(|prefix| file_name.starts_with(prefix))
            })
        })
}

fn extract_with(compiled: &[Regex], patterns: &[Pattern], content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        for (regex, pattern) in compiled.iter().zip(patterns) {
            let Some(captures) = regex.captures(line) else {
                continue;
            };
            let kind = match pattern.kind {
                Kind::Fixed(kind) => kind.to_owned(),
                Kind::Captured(group) => captures[group].to_owned(),
            };
            let name = match pattern.name {
                Name::Capture(group) => captures[group].to_owned(),
                Name::Dotted { head, tail } => captures
                    .get(tail)
                    .map(|tail| format!("{}.{}", &captures[head], tail.as_str()))
                    .unwrap_or_else(|| captures[head].to_owned()),
                Name::Preferred { first, fallback } => captures
                    .get(first)
                    .or_else(|| captures.get(fallback))
                    .map(|value| value.as_str().to_owned())
                    .unwrap_or_default(),
            };
            push_symbol(&mut symbols, index + 1, &kind, &name);
            // First pattern to match a line wins, as it did before.
            break;
        }
    }
    symbols
}

fn is_markdown(extension: &str) -> bool {
    matches!(extension, "md" | "markdown")
}

/// Heading depth is counted rather than captured, so this one stays a function.
fn extract_markdown_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') {
            continue;
        }
        let level = trimmed.chars().take_while(|char| *char == '#').count();
        let name = trimmed[level..].trim();
        if !name.is_empty() {
            push_symbol(&mut symbols, index + 1, &format!("h{level}"), name);
        }
    }
    symbols
}

/// The fallback for an unrecognised file: no regex, and the whole trimmed line
/// is the name.
fn extract_generic_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("class ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("def ")
        {
            push_symbol(&mut symbols, index + 1, "symbol", trimmed);
        }
    }
    symbols
}

fn push_symbol(symbols: &mut Vec<Symbol>, line: usize, kind: &str, name: &str) {
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    symbols.push(Symbol {
        line,
        kind: kind.to_owned(),
        name: name.to_owned(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row has no name field - nothing in production would read it - so a
    /// failure names the spec by the first thing that dispatches to it.
    fn describe(spec: &LanguageSpec) -> &'static str {
        spec.extensions
            .first()
            .or_else(|| spec.filenames.first())
            .or_else(|| spec.filename_prefixes.first())
            .copied()
            .unwrap_or("<unreachable>")
    }

    /// A table is only trustworthy if every row is exercised. The snapshot
    /// corpus is what exercises it, so this fails on a row no fixture reaches -
    /// which is also what would happen to a newly added language whose author
    /// forgot the fixture.
    #[test]
    fn every_pattern_is_reachable_from_the_snapshot_corpus() {
        let lines: Vec<&str> = SYMBOL_FIXTURES
            .iter()
            .flat_map(|(_, content)| content.lines())
            .collect();

        let mut unreached = Vec::new();
        for (spec, compiled) in LANGUAGES.iter().zip(COMPILED.iter()) {
            for (index, regex) in compiled.iter().enumerate() {
                if !lines.iter().any(|line| regex.is_match(line)) {
                    unreached.push(format!("{}[{index}]", describe(spec)));
                }
            }
        }

        assert!(
            unreached.is_empty(),
            "table rows no fixture reaches: {unreached:?}"
        );
    }

    #[test]
    fn every_language_is_reachable_from_a_file_name() {
        for spec in LANGUAGES {
            assert!(
                !spec.extensions.is_empty()
                    || !spec.filenames.is_empty()
                    || !spec.filename_prefixes.is_empty(),
                "{} can never be dispatched to",
                describe(spec)
            );
        }
    }

    #[test]
    fn markdown_stays_out_of_the_table() {
        assert!(is_markdown("md"));
        assert!(is_markdown("markdown"));
        assert!(
            LANGUAGES
                .iter()
                .all(|spec| !spec.extensions.contains(&"md")),
            "markdown counts heading depth and cannot be a pattern row"
        );
    }
}
