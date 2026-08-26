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

static LANGUAGES: &[LanguageSpec] = &[
    // rust
    LanguageSpec {
        extensions: &["rs"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(pub\s+)?(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("fn"),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(pub\s+)?(struct|enum|trait)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*impl(\s*<[^>]+>)?\s+([A-Za-z_][A-Za-z0-9_:<>]*)",
                kind: Kind::Fixed("impl"),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("mod"),
                name: Name::Capture(2),
            },
        ],
    },
    // python
    LanguageSpec {
        extensions: &["py"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("class"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("def"),
                name: Name::Capture(2),
            },
        ],
    },
    // js_ts
    LanguageSpec {
        extensions: &["js", "jsx", "ts", "tsx", "vue"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(export\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("class"),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("function"),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(export\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("interface"),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(export\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)\s*=",
                kind: Kind::Fixed("type"),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(export\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(async\s*)?\(",
                kind: Kind::Fixed("const-fn"),
                name: Name::Capture(2),
            },
        ],
    },
    // go
    LanguageSpec {
        extensions: &["go"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*func\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("func"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+",
                kind: Kind::Fixed("type"),
                name: Name::Capture(1),
            },
        ],
    },
    // java_like
    LanguageSpec {
        extensions: &["java"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)",
                kind: Kind::Fixed("package"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(public\s+|private\s+|protected\s+)?(class|interface|enum|record)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(public|private|protected)\s+(static\s+)?[A-Za-z_][A-Za-z0-9_<>,\[\]?]*\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
                kind: Kind::Fixed("method"),
                name: Name::Capture(3),
            },
        ],
    },
    // kotlin
    LanguageSpec {
        extensions: &["kt", "kts"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)",
                kind: Kind::Fixed("package"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(data\s+|sealed\s+|open\s+)?(class|interface|object|enum class)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(public\s+|private\s+|protected\s+)?fun\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("fun"),
                name: Name::Capture(2),
            },
        ],
    },
    // scala
    LanguageSpec {
        extensions: &["scala"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)",
                kind: Kind::Fixed("package"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(case\s+)?(class|trait|object|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(override\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("def"),
                name: Name::Capture(2),
            },
        ],
    },
    // csharp
    LanguageSpec {
        extensions: &["cs"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_.]*)",
                kind: Kind::Fixed("namespace"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(public\s+|internal\s+|private\s+|protected\s+)?(class|interface|enum|struct|record)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(public|private|protected|internal)\s+(static\s+|async\s+)*[A-Za-z_][A-Za-z0-9_<>,\[\]?]*\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
                kind: Kind::Fixed("method"),
                name: Name::Capture(3),
            },
        ],
    },
    // php
    LanguageSpec {
        extensions: &["php"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*namespace\s+([A-Za-z_\\][A-Za-z0-9_\\]*)",
                kind: Kind::Fixed("namespace"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(abstract\s+|final\s+)?(class|interface|trait|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(public\s+|private\s+|protected\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("function"),
                name: Name::Capture(2),
            },
        ],
    },
    // ruby
    LanguageSpec {
        extensions: &["rb"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(class|module)\s+([A-Za-z_][A-Za-z0-9_:]*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*def\s+([A-Za-z_][A-Za-z0-9_!?=.]*)",
                kind: Kind::Fixed("def"),
                name: Name::Capture(1),
            },
        ],
    },
    // elixir
    LanguageSpec {
        extensions: &["ex", "exs"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*defmodule\s+([A-Za-z_][A-Za-z0-9_.]*)",
                kind: Kind::Fixed("defmodule"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(def|defp|defmacro)\s+([A-Za-z_][A-Za-z0-9_!?]*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
        ],
    },
    // erlang
    LanguageSpec {
        extensions: &["erl", "hrl"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*-module\(([a-zA-Z0-9_@]+)\)",
                kind: Kind::Fixed("module"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*([a-z][A-Za-z0-9_@]*)\s*\([^;]*\)\s*->",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // swift
    LanguageSpec {
        extensions: &["swift"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(public\s+|private\s+|internal\s+|open\s+)?(class|struct|enum|protocol|actor)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(2),
                name: Name::Capture(3),
            },
            Pattern {
                regex: r"^\s*(public\s+|private\s+|internal\s+)?func\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("func"),
                name: Name::Capture(2),
            },
        ],
    },
    // dart
    LanguageSpec {
        extensions: &["dart"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(class|enum|mixin|extension)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(?:[A-Za-z_][A-Za-z0-9_<>,?]*\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // c_cpp
    LanguageSpec {
        extensions: &["c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_:]*)",
                kind: Kind::Fixed("namespace"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(class|struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*(?:[A-Za-z_][A-Za-z0-9_:<>,*&\s]+)\s+([A-Za-z_][A-Za-z0-9_:]*)\s*\([^;]*\)\s*(?:\{|$)",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // zig
    LanguageSpec {
        extensions: &["zig"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("fn"),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*const\s+([A-Za-z_][A-Za-z0-9_]*)\s*=",
                kind: Kind::Fixed("const"),
                name: Name::Capture(1),
            },
        ],
    },
    // lua
    LanguageSpec {
        extensions: &["lua"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r"^\s*(?:local\s+)?function\s+([A-Za-z_][A-Za-z0-9_:.]*)",
            kind: Kind::Fixed("function"),
            name: Name::Capture(1),
        }],
    },
    // perl
    LanguageSpec {
        extensions: &["pl", "pm"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)",
                kind: Kind::Fixed("package"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Fixed("sub"),
                name: Name::Capture(1),
            },
        ],
    },
    // r
    LanguageSpec {
        extensions: &["r"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r"^\s*([A-Za-z.][A-Za-z0-9._]*)\s*(?:<-|=)\s*function\s*\(",
            kind: Kind::Fixed("function"),
            name: Name::Capture(1),
        }],
    },
    // julia
    LanguageSpec {
        extensions: &["jl"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(module|struct|mutable struct|abstract type)\s+([A-Za-z_][A-Za-z0-9_]*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*function\s+([A-Za-z_][A-Za-z0-9_!.]*)",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // haskell
    LanguageSpec {
        extensions: &["hs"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*module\s+([A-Za-z_][A-Za-z0-9_.']*)",
                kind: Kind::Fixed("module"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*(data|newtype|type|class)\s+([A-Z][A-Za-z0-9_']*)",
                kind: Kind::Captured(1),
                name: Name::Capture(2),
            },
            Pattern {
                regex: r"^\s*([a-z_][A-Za-z0-9_']*)\s*::",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // ocaml
    LanguageSpec {
        extensions: &["ml", "mli"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*module\s+([A-Z][A-Za-z0-9_']*)",
                kind: Kind::Fixed("module"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*type\s+([a-zA-Z_][A-Za-z0-9_']*)",
                kind: Kind::Fixed("type"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*let\s+(?:rec\s+)?([a-z_][A-Za-z0-9_']*)",
                kind: Kind::Fixed("let"),
                name: Name::Capture(1),
            },
        ],
    },
    // yaml
    LanguageSpec {
        extensions: &["yml", "yaml"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r"^([A-Za-z_][A-Za-z0-9_-]*)\s*:",
            kind: Kind::Fixed("key"),
            name: Name::Capture(1),
        }],
    },
    // toml
    LanguageSpec {
        extensions: &["toml"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r"^\s*\[+([A-Za-z0-9_.-]+)\]+",
            kind: Kind::Fixed("section"),
            name: Name::Capture(1),
        }],
    },
    // shell
    LanguageSpec {
        extensions: &["sh", "bash", "zsh", "ps1", "psm1"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[
            Pattern {
                regex: r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_-]*)\s*(?:\(\))\s*\{",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
            Pattern {
                regex: r"^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)",
                kind: Kind::Fixed("function"),
                name: Name::Capture(1),
            },
        ],
    },
    // taskfile
    LanguageSpec {
        extensions: &[],
        filenames: &["makefile", "justfile", "rakefile"],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r"^([A-Za-z0-9_.-]+)\s*:",
            kind: Kind::Fixed("target"),
            name: Name::Capture(1),
        }],
    },
    // terraform
    LanguageSpec {
        extensions: &["tf", "tofu"],
        filenames: &[],
        filename_prefixes: &[],
        patterns: &[Pattern {
            regex: r#"^\s*(resource|data|module|variable|output|provider|locals)\s+"([^"]+)"(?:\s+"([^"]+)")?"#,
            kind: Kind::Captured(1),
            name: Name::Dotted { head: 2, tail: 3 },
        }],
    },
    // dockerfile
    LanguageSpec {
        extensions: &[],
        filenames: &["dockerfile"],
        filename_prefixes: &["dockerfile."],
        patterns: &[Pattern {
            regex: r"(?i)^\s*FROM\s+([^\s]+)(?:\s+AS\s+([A-Za-z_][A-Za-z0-9_-]*))?",
            kind: Kind::Fixed("stage"),
            name: Name::Preferred {
                first: 2,
                fallback: 1,
            },
        }],
    },
];

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

/// One fixture per dispatch arm of `symbols::extract_symbols`, covering
/// every extraction pattern the module defines.
///
/// The corpus was checked mechanically: each of the 61 regexes in that module
/// matches at least one line here. That matters because the module had no unit
/// tests of its own and the integration tests assert only that a handful of
/// expected symbols are *present*, never that the whole extraction is
/// unchanged.
///
/// Lives here rather than with the snapshot that renders it, because a table
/// gaining an arm has to gain a fixture in the same edit.
///
/// Behind a feature rather than `cfg(test)`: the golden snapshot that renders it
/// lives in the CLI crate, and one crate's test configuration is invisible to
/// another's. A release build enables no dev-dependencies, so it compiles none
/// of this.
#[cfg(any(test, feature = "fixtures"))]
pub const SYMBOL_FIXTURES: &[(&str, &str)] = &[
    (
        "lib.rs",
        "pub async fn build_index(root: &Path) -> Result<()> {\nstruct Config {\npub enum Mode {\ntrait Render {\nimpl<T: Clone> Render for Config {\npub mod helpers {\n",
    ),
    (
        "README.md",
        "# Title\n## Section\n### Sub section\n#### Deep\n",
    ),
    (
        "app.py",
        "class Service:\n    async def handle(self):\n    def plain(self):\n",
    ),
    (
        "app.ts",
        "export class Widget {\nexport async function render() {\nexport interface Props {\nexport type Alias = string;\nexport const build = async (\n",
    ),
    ("main.go", "func Serve() {\ntype Server struct {\n"),
    (
        "Main.java",
        "package com.example.demo;\npublic sealed interface Service {\npublic class App {\n    public static String render(int value) {\n",
    ),
    (
        "App.kt",
        "package com.example.demo\ndata class Person(val name: String)\nprivate fun boot() {\n",
    ),
    (
        "App.scala",
        "package com.example\ncase class Item(name: String)\noverride def run(): Unit = {\n",
    ),
    (
        "Program.cs",
        "namespace Demo.App;\npublic record Item(string Name);\npublic static string Render(int value) {\n",
    ),
    (
        "lib.php",
        "namespace App\\Domain;\nabstract class Handler {\nfunction handle() {\n",
    ),
    ("worker.rb", "module Demo\nclass Worker\n  def perform!\n"),
    (
        "app.ex",
        "defmodule Demo.Worker do\n  def perform do\n  defp helper? do\n  defmacro guarded do\n",
    ),
    ("app.erl", "-module(demo_worker).\nhandle(Request) ->\n"),
    (
        "App.swift",
        "public struct Model {\ninternal actor Store {\nprivate func reload() {\n",
    ),
    (
        "main.dart",
        "class Widget {\nmixin Logging {\nvoid render(\n",
    ),
    (
        "main.cpp",
        "namespace demo::core {\nclass Engine {\nstruct Point {\nint compute(int value) {\n",
    ),
    (
        "main.zig",
        "pub fn main() void {\nconst Config = struct {\n",
    ),
    (
        "init.lua",
        "local function setup()\nfunction M.teardown()\n",
    ),
    ("Module.pm", "package Demo::Module;\nsub render {\n"),
    (
        "analysis.r",
        "summarise <- function(data) {\nplot.data = function(x) {\n",
    ),
    (
        "model.jl",
        "struct Point\nmutable struct Buffer\nfunction solve(x)\n",
    ),
    (
        "Lib.hs",
        "module Demo.Lib where\ndata Shape = Circle\nnewtype Wrapper = Wrapper Int\nclass Render a where\nrender :: Shape -> String\n",
    ),
    (
        "lib.ml",
        "module Store = struct\ntype shape = Circle\nlet rec walk node =\n",
    ),
    (
        "main.tf",
        "resource \"aws_s3_bucket\" \"logs\" {\nmodule \"network\" {\nvariable \"region\" {\n",
    ),
    ("config.yml", "service:\nimage_name:\n"),
    ("Cargo.toml", "[package]\n[[bin]]\n[dependencies.serde]\n"),
    ("deploy.sh", "function build() {\nteardown() {\n"),
    ("Module.psm1", "function Invoke-Build {\n"),
    (
        "Dockerfile",
        "FROM rust:1.88 AS builder\nFROM debian:bookworm\n",
    ),
    ("Makefile", "build:\ntest-all:\n"),
    (
        "notes.txt",
        "class Loose\ninterface Loose\nfn loose\ndef loose\n",
    ),
];

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
