//! Drift checks between `docs/reference/*.md` and the commands it documents.
//!
//! Generating the reference pages was the original plan, but they are prose:
//! per-domain notes, error-code tables, tool-resolution order, security
//! warnings. A generator would rewrite all of that into a schema dump. What was
//! actually missing is the tie between the prose and the CLI, so nothing checked
//! that a documented command still exists or that a new one got documented at
//! all — and the pages had no test of any kind.
//!
//! Both checks read the checked-in snapshots rather than the live catalogs: the
//! four plugin catalogs live in separately compiled cdylibs that the host cannot
//! link, and every one of these files is already verified against its live
//! source by its own snapshot test.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|error| panic!("{relative} should be readable: {error}"))
}

fn parse(relative: &str) -> Value {
    serde_json::from_str(&read(relative))
        .unwrap_or_else(|error| panic!("{relative} should be JSON: {error}"))
}

/// The plugin catalogs the host cannot link, keyed by the snapshot each one
/// checks in.
const PLUGIN_CATALOGS: &[&str] = &[
    "plugins/ah-plugin-github/tests/snapshots/command-catalog.snap",
    "plugins/ah-plugin-gitlab/tests/snapshots/command-catalog.snap",
    "plugins/ah-plugin-ollama/tests/snapshots/command-catalog.snap",
    "plugins/ah-plugin-postgres/tests/snapshots/command-catalog.snap",
];

/// Every invocation a user can type, as `ah <path>`: the typed commands of the
/// built-in domains and of the four dynamic plugins, plus the host's own clap
/// commands, which are not typed commands and so appear only in the help tree.
fn command_invocations() -> BTreeSet<String> {
    let mut invocations = BTreeSet::new();

    let built_ins = parse("tests/snapshots/typed-command-catalog.snap");
    for entry in built_ins.as_array().expect("catalog should be an array") {
        invocations.insert(invocation(command_id(&entry["descriptor"])));
    }

    for catalog in PLUGIN_CATALOGS {
        let catalog = parse(catalog);
        for command in catalog["commands"]
            .as_array()
            .expect("plugin catalog should list commands")
        {
            invocations.insert(invocation(command_id(command)));
        }
    }

    for line in read("tests/snapshots/cli-help.snap").lines() {
        let Some(path) = line
            .strip_prefix("$ ")
            .and_then(|l| l.strip_suffix(" --help"))
        else {
            continue;
        };
        // The root, a bare domain, and clap's generated `help` are not commands
        // the reference pages document.
        if path.split_whitespace().count() < 3 || path.ends_with(" help") {
            continue;
        }
        invocations.insert(path.to_owned());
    }

    invocations
}

fn command_id(descriptor: &Value) -> &str {
    descriptor["id"]
        .as_str()
        .expect("command should have an id")
}

fn invocation(command_id: &str) -> String {
    format!("ah {}", command_id.replace('.', " "))
}

/// The domain owning an invocation, which is also the stem of its page.
fn domain_of(invocation: &str) -> &str {
    invocation
        .split_whitespace()
        .nth(1)
        .expect("an invocation names a domain")
}

#[test]
fn every_command_is_documented() {
    let mut undocumented = Vec::new();
    for invocation in command_invocations() {
        let page = format!("docs/reference/{}.md", domain_of(&invocation));
        let Ok(text) = fs::read_to_string(repo_path(&page)) else {
            undocumented.push(format!("{invocation} (no {page})"));
            continue;
        };
        if !text.contains(&invocation) {
            undocumented.push(format!("{invocation} (not named in {page})"));
        }
    }

    assert!(
        undocumented.is_empty(),
        "commands missing from docs/reference: {undocumented:#?}"
    );
}

#[test]
fn every_documented_command_exists() {
    let known = command_invocations();
    let mut unknown = Vec::new();

    let pages =
        fs::read_dir(repo_path("docs/reference")).expect("reference directory should exist");
    for page in pages {
        let path = page.expect("directory entry should be readable").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("page should have a name")
            .to_owned();
        let text = fs::read_to_string(&path).expect("page should be readable");

        for heading in text.lines().filter_map(command_heading) {
            for invocation in expand_alternatives(heading) {
                if !known.contains(&invocation) {
                    unknown.push(format!("{name}: `{invocation}`"));
                }
            }
        }
    }

    assert!(
        unknown.is_empty(),
        "documented commands that do not exist: {unknown:#?}"
    );
}

/// A command section is headed by its invocation, as ``## `ah git tags` ``.
fn command_heading(line: &str) -> Option<&str> {
    line.strip_prefix("## `ah ")
        .and_then(|rest| rest.strip_suffix('`'))
        .map(|rest| rest.trim())
        .filter(|rest| !rest.is_empty())
}

/// A page may head one section for sibling commands, as `ah http
/// get|post|put|patch|delete`.
fn expand_alternatives(heading: &str) -> Vec<String> {
    let segments: Vec<&str> = heading.split_whitespace().collect();
    let Some((last, prefix)) = segments.split_last() else {
        return Vec::new();
    };
    let prefix = prefix.join(" ");
    last.split('|')
        .map(|alternative| {
            if prefix.is_empty() {
                format!("ah {alternative}")
            } else {
                format!("ah {prefix} {alternative}")
            }
        })
        .collect()
}

/// Every Markdown file in the repository, excluding build output and VCS data.
fn markdown_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                    walk(&path, found);
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                found.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(&repo_path(""), &mut found);
    found.sort();
    found
}

/// The path a reference points at, or `None` when it is not one this check can
/// resolve: an external URL, a bare anchor, or a spelling that stands for a
/// family of files rather than one of them.
fn resolvable(target: &str) -> Option<&str> {
    if target.contains("://") || target.starts_with("mailto:") {
        return None;
    }
    let target = target.split('#').next().unwrap_or(target);
    // A `path.rs:40` citation points at a line of the file, not at a path.
    let target = match target.rsplit_once(':') {
        Some((path, line))
            if !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            path
        }
        _ => target,
    };
    if target.is_empty() || target.contains('*') || target.contains('<') || target.contains(' ') {
        return None;
    }
    Some(target)
}

/// `](target)` — the inline link and image form, which is the only one this
/// repository uses.
fn markdown_link_targets(text: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        if let Some(close) = rest.find(')') {
            targets.push(&rest[..close]);
            rest = &rest[close..];
        }
    }
    targets
}

/// A backticked path into the repository, as ``` `crates/ah-domains/src/git.rs` ```.
/// Only the directories that hold code and documentation count, so a backticked
/// `plugins.json` or `Cargo.toml` is not mistaken for one.
///
/// The dot-directories are deliberately absent: `docs/reference/ai.md` names
/// `.github/copilot-instructions.md` and `.cursor/rules/ah.mdc` as paths in the
/// *user's* project, which this repository has no opinion about.
fn inline_repository_paths(text: &str) -> Vec<&str> {
    const ROOTS: &[&str] = &[
        "src/", "crates/", "plugins/", "tests/", "docs/", "scripts/", ".agents/",
    ];

    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|span| ROOTS.iter().any(|root| span.starts_with(root)))
        .collect()
}

#[test]
fn documentation_points_at_files_that_exist() {
    let root = repo_path("");
    let mut dangling = Vec::new();

    for page in markdown_files() {
        // The changelog records what the repository used to contain, so its
        // paths are history rather than claims about the tree today.
        if page.file_name().and_then(|name| name.to_str()) == Some("CHANGELOG.md") {
            continue;
        }
        let directory = page.parent().expect("a file has a parent").to_owned();
        let relative = page
            .strip_prefix(&root)
            .unwrap_or(&page)
            .display()
            .to_string();
        let text = fs::read_to_string(&page).expect("page should be readable");

        let links = markdown_link_targets(&text)
            .into_iter()
            .filter_map(resolvable)
            .map(|target| (directory.join(target), target));
        let paths = inline_repository_paths(&text)
            .into_iter()
            .filter_map(resolvable)
            .map(|target| (root.join(target), target));

        for (resolved, target) in links.chain(paths) {
            if !resolved.exists() {
                dangling.push(format!("{relative}: {target}"));
            }
        }
    }

    dangling.sort();
    dangling.dedup();
    assert!(
        dangling.is_empty(),
        "documentation points at files that do not exist: {dangling:#?}"
    );
}
