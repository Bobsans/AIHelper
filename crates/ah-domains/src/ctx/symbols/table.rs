//! One entry per language the extractor knows, and its patterns.
//!
//! Data, and nothing else. It lives in its own file because it is 520 lines
//! long and adding a language means adding one row here - the `Pattern`, `Kind`
//! and `Name` types it uses, and the extraction that walks it, are all in
//! `symbols.rs`, which is what a reader actually has to understand.
//!
//! Every regex here is reachable from the fixture corpus, which a test in
//! `symbols.rs` checks: a pattern nothing matches is dead weight that still
//! reads as coverage.

use super::*;

pub(super) static LANGUAGES: &[LanguageSpec] = &[
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
