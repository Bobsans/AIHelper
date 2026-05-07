use std::{path::Path, sync::OnceLock};

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub line: usize,
    pub kind: String,
    pub name: String,
}

pub fn extract_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "rs" => extract_rust_symbols(content),
        "md" | "markdown" => extract_markdown_symbols(content),
        "py" => extract_python_symbols(content),
        "js" | "jsx" | "ts" | "tsx" | "vue" => extract_js_ts_symbols(content),
        "go" => extract_go_symbols(content),
        "java" => extract_java_like_symbols(content),
        "kt" | "kts" => extract_kotlin_symbols(content),
        "scala" => extract_scala_symbols(content),
        "cs" => extract_csharp_symbols(content),
        "php" => extract_php_symbols(content),
        "rb" => extract_ruby_symbols(content),
        "ex" | "exs" => extract_elixir_symbols(content),
        "erl" | "hrl" => extract_erlang_symbols(content),
        "swift" => extract_swift_symbols(content),
        "dart" => extract_dart_symbols(content),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => extract_c_cpp_symbols(content),
        "zig" => extract_zig_symbols(content),
        "lua" => extract_lua_symbols(content),
        "pl" | "pm" => extract_perl_symbols(content),
        "r" => extract_r_symbols(content),
        "jl" => extract_julia_symbols(content),
        "hs" => extract_haskell_symbols(content),
        "ml" | "mli" => extract_ocaml_symbols(content),
        "tf" | "tofu" => extract_terraform_symbols(content),
        "yml" | "yaml" => extract_yaml_symbols(content),
        "toml" => extract_toml_symbols(content),
        "sh" | "bash" | "zsh" | "ps1" | "psm1" => extract_shell_symbols(content),
        _ if file_name == "dockerfile" || file_name.starts_with("dockerfile.") => {
            extract_dockerfile_symbols(content)
        }
        _ if matches!(file_name.as_str(), "makefile" | "justfile" | "rakefile") => {
            extract_taskfile_symbols(content)
        }
        _ => extract_generic_symbols(content),
    }
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

fn extract_rust_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = rust_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "fn", &captures[3]);
            continue;
        }
        if let Some(captures) = rust_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = rust_impl_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "impl", &captures[2]);
            continue;
        }
        if let Some(captures) = rust_mod_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "mod", &captures[2]);
        }
    }
    symbols
}

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

fn extract_python_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = python_class_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "class", &captures[1]);
            continue;
        }
        if let Some(captures) = python_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "def", &captures[2]);
        }
    }
    symbols
}

fn extract_js_ts_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = js_class_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "class", &captures[2]);
            continue;
        }
        if let Some(captures) = js_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[3]);
            continue;
        }
        if let Some(captures) = js_interface_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "interface", &captures[2]);
            continue;
        }
        if let Some(captures) = js_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "type", &captures[2]);
            continue;
        }
        if let Some(captures) = js_const_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "const-fn", &captures[2]);
        }
    }
    symbols
}

fn extract_go_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = go_func_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "func", &captures[1]);
            continue;
        }
        if let Some(captures) = go_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "type", &captures[1]);
        }
    }
    symbols
}

fn extract_java_like_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = java_package_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "package", &captures[1]);
            continue;
        }
        if let Some(captures) = java_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = java_method_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "method", &captures[3]);
        }
    }
    symbols
}

fn extract_kotlin_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = java_package_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "package", &captures[1]);
            continue;
        }
        if let Some(captures) = kotlin_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = kotlin_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "fun", &captures[2]);
        }
    }
    symbols
}

fn extract_scala_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = scala_package_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "package", &captures[1]);
            continue;
        }
        if let Some(captures) = scala_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = scala_def_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "def", &captures[2]);
        }
    }
    symbols
}

fn extract_csharp_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = csharp_namespace_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "namespace", &captures[1]);
            continue;
        }
        if let Some(captures) = csharp_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = csharp_method_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "method", &captures[3]);
        }
    }
    symbols
}

fn extract_php_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = php_namespace_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "namespace", &captures[1]);
            continue;
        }
        if let Some(captures) = php_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = php_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[2]);
        }
    }
    symbols
}

fn extract_ruby_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = ruby_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
            continue;
        }
        if let Some(captures) = ruby_def_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "def", &captures[1]);
        }
    }
    symbols
}

fn extract_elixir_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = elixir_module_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "defmodule", &captures[1]);
            continue;
        }
        if let Some(captures) = elixir_def_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
        }
    }
    symbols
}

fn extract_erlang_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = erlang_module_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "module", &captures[1]);
            continue;
        }
        if let Some(captures) = erlang_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_swift_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = swift_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[2], &captures[3]);
            continue;
        }
        if let Some(captures) = swift_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "func", &captures[2]);
        }
    }
    symbols
}

fn extract_dart_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = dart_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
            continue;
        }
        if let Some(captures) = dart_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_c_cpp_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = cpp_namespace_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "namespace", &captures[1]);
            continue;
        }
        if let Some(captures) = cpp_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
            continue;
        }
        if let Some(captures) = cpp_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_zig_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = zig_fn_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "fn", &captures[2]);
            continue;
        }
        if let Some(captures) = zig_const_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "const", &captures[1]);
        }
    }
    symbols
}

fn extract_lua_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = lua_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_perl_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = perl_package_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "package", &captures[1]);
            continue;
        }
        if let Some(captures) = perl_sub_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "sub", &captures[1]);
        }
    }
    symbols
}

fn extract_r_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = r_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_julia_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = julia_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
            continue;
        }
        if let Some(captures) = julia_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_haskell_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = haskell_module_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "module", &captures[1]);
            continue;
        }
        if let Some(captures) = haskell_decl_re().captures(line) {
            push_symbol(&mut symbols, index + 1, &captures[1], &captures[2]);
            continue;
        }
        if let Some(captures) = haskell_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_ocaml_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = ocaml_module_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "module", &captures[1]);
            continue;
        }
        if let Some(captures) = ocaml_type_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "type", &captures[1]);
            continue;
        }
        if let Some(captures) = ocaml_let_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "let", &captures[1]);
        }
    }
    symbols
}

fn extract_terraform_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = terraform_block_re().captures(line) {
            let kind = &captures[1];
            let name = captures
                .get(3)
                .map(|name| format!("{}.{}", &captures[2], name.as_str()))
                .unwrap_or_else(|| captures[2].to_owned());
            push_symbol(&mut symbols, index + 1, kind, &name);
        }
    }
    symbols
}

fn extract_yaml_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = yaml_key_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "key", &captures[1]);
        }
    }
    symbols
}

fn extract_toml_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = toml_section_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "section", &captures[1]);
        }
    }
    symbols
}

fn extract_shell_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = shell_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
            continue;
        }
        if let Some(captures) = powershell_function_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "function", &captures[1]);
        }
    }
    symbols
}

fn extract_dockerfile_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = docker_stage_re().captures(line) {
            let name = captures
                .get(2)
                .or_else(|| captures.get(1))
                .map(|value| value.as_str())
                .unwrap_or_default();
            push_symbol(&mut symbols, index + 1, "stage", name);
        }
    }
    symbols
}

fn extract_taskfile_symbols(content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = task_target_re().captures(line) {
            push_symbol(&mut symbols, index + 1, "target", &captures[1]);
        }
    }
    symbols
}

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

fn rust_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(pub\s+)?(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn rust_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(pub\s+)?(struct|enum|trait)\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn rust_impl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*impl(\s*<[^>]+>)?\s+([A-Za-z_][A-Za-z0-9_:<>]*)"))
}

fn rust_mod_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn python_class_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn python_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn js_class_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(export\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn js_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn js_interface_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(export\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn js_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(export\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)\s*="))
}

fn js_const_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(export\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(async\s*)?\(")
    })
}

fn go_func_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*func\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn go_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+"))
}

fn java_package_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)"))
}

fn java_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(public\s+|private\s+|protected\s+)?(class|interface|enum|record)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
    })
}

fn java_method_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(public|private|protected)\s+(static\s+)?[A-Za-z_][A-Za-z0-9_<>,\[\]?]*\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
    })
}

fn kotlin_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(data\s+|sealed\s+|open\s+)?(class|interface|object|enum class)\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn kotlin_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(public\s+|private\s+|protected\s+)?fun\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn scala_package_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_.]*)"))
}

fn scala_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(case\s+)?(class|trait|object|enum)\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn scala_def_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(override\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn csharp_namespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_.]*)"))
}

fn csharp_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(public\s+|internal\s+|private\s+|protected\s+)?(class|interface|enum|struct|record)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
    })
}

fn csharp_method_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(public|private|protected|internal)\s+(static\s+|async\s+)*[A-Za-z_][A-Za-z0-9_<>,\[\]?]*\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
    })
}

fn php_namespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*namespace\s+([A-Za-z_\\][A-Za-z0-9_\\]*)"))
}

fn php_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(abstract\s+|final\s+)?(class|interface|trait|enum)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
    })
}

fn php_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(public\s+|private\s+|protected\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
    })
}

fn ruby_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(class|module)\s+([A-Za-z_][A-Za-z0-9_:]*)"))
}

fn ruby_def_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*def\s+([A-Za-z_][A-Za-z0-9_!?=.]*)"))
}

fn elixir_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*defmodule\s+([A-Za-z_][A-Za-z0-9_.]*)"))
}

fn elixir_def_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(def|defp|defmacro)\s+([A-Za-z_][A-Za-z0-9_!?]*)"))
}

fn erlang_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*-module\(([a-zA-Z0-9_@]+)\)"))
}

fn erlang_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*([a-z][A-Za-z0-9_@]*)\s*\([^;]*\)\s*->"))
}

fn swift_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(public\s+|private\s+|internal\s+|open\s+)?(class|struct|enum|protocol|actor)\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn swift_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(public\s+|private\s+|internal\s+)?func\s+([A-Za-z_][A-Za-z0-9_]*)")
    })
}

fn dart_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(class|enum|mixin|extension)\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn dart_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(?:[A-Za-z_][A-Za-z0-9_<>,?]*\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(")
    })
}

fn cpp_namespace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*namespace\s+([A-Za-z_][A-Za-z0-9_:]*)"))
}

fn cpp_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(class|struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn cpp_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(?:[A-Za-z_][A-Za-z0-9_:<>,*&\s]+)\s+([A-Za-z_][A-Za-z0-9_:]*)\s*\([^;]*\)\s*(?:\{|$)")
    })
}

fn zig_fn_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn zig_const_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*const\s+([A-Za-z_][A-Za-z0-9_]*)\s*="))
}

fn lua_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(?:local\s+)?function\s+([A-Za-z_][A-Za-z0-9_:.]*)"))
}

fn perl_package_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)"))
}

fn perl_sub_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)"))
}

fn r_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*([A-Za-z.][A-Za-z0-9._]*)\s*(?:<-|=)\s*function\s*\("))
}

fn julia_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(
            r"^\s*(module|struct|mutable struct|abstract type)\s+([A-Za-z_][A-Za-z0-9_]*)",
        )
    })
}

fn julia_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*function\s+([A-Za-z_][A-Za-z0-9_!.]*)"))
}

fn haskell_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*module\s+([A-Za-z_][A-Za-z0-9_.']*)"))
}

fn haskell_decl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*(data|newtype|type|class)\s+([A-Z][A-Za-z0-9_']*)"))
}

fn haskell_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*([a-z_][A-Za-z0-9_']*)\s*::"))
}

fn ocaml_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*module\s+([A-Z][A-Za-z0-9_']*)"))
}

fn ocaml_type_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*type\s+([a-zA-Z_][A-Za-z0-9_']*)"))
}

fn ocaml_let_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*let\s+(?:rec\s+)?([a-z_][A-Za-z0-9_']*)"))
}

fn terraform_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r#"^\s*(resource|data|module|variable|output|provider|locals)\s+"([^"]+)"(?:\s+"([^"]+)")?"#)
    })
}

fn yaml_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^([A-Za-z_][A-Za-z0-9_-]*)\s*:"))
}

fn toml_section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*\[+([A-Za-z0-9_.-]+)\]+"))
}

fn shell_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"^\s*(?:function\s+)?([A-Za-z_][A-Za-z0-9_-]*)\s*(?:\(\))\s*\{")
    })
}

fn powershell_function_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)"))
}

fn docker_stage_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        compile_regex(r"(?i)^\s*FROM\s+([^\s]+)(?:\s+AS\s+([A-Za-z_][A-Za-z0-9_-]*))?")
    })
}

fn task_target_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| compile_regex(r"^([A-Za-z0-9_.-]+)\s*:"))
}

fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid ctx symbol regex")
}
