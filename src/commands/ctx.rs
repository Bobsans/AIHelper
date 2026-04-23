use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use clap::{Args, Subcommand};
use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

use crate::{cli::GlobalOptions, error::AppError, output::OutputMode};

#[derive(Debug, Args)]
pub struct CtxArgs {
    #[command(subcommand)]
    pub command: CtxCommand,
}

#[derive(Debug, Subcommand)]
pub enum CtxCommand {
    Pack(PackArgs),
    Symbols(SymbolsArgs),
    Changed(ChangedArgs),
}

#[derive(Debug, Args)]
pub struct PackArgs {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Args)]
pub struct SymbolsArgs {
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ChangedArgs {}

#[derive(Debug, Clone, Serialize)]
struct Symbol {
    line: usize,
    kind: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct SymbolsFileOutput {
    path: String,
    symbol_count: usize,
    symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize)]
struct CtxSymbolsOutput {
    command: &'static str,
    root: String,
    file_count: usize,
    symbol_count: usize,
    truncated: bool,
    files: Vec<SymbolsFileOutput>,
}

#[derive(Debug, Serialize)]
struct PackItem {
    path: String,
    kind: String,
    size_bytes: u64,
    line_count: usize,
    symbol_count: usize,
    symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize)]
struct CtxPackOutput {
    command: &'static str,
    roots: Vec<String>,
    item_count: usize,
    file_count: usize,
    directory_count: usize,
    symbol_count: usize,
    truncated: bool,
    items: Vec<PackItem>,
}

#[derive(Debug, Serialize)]
struct ChangedEntry {
    status: String,
    path: String,
    old_path: Option<String>,
}

#[derive(Debug, Serialize)]
struct CtxChangedOutput {
    command: &'static str,
    in_git_repo: bool,
    changed_count: usize,
    entries: Vec<ChangedEntry>,
}

pub fn execute(args: CtxArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        CtxCommand::Pack(pack_args) => execute_pack(pack_args, options),
        CtxCommand::Symbols(symbols_args) => execute_symbols(symbols_args, options),
        CtxCommand::Changed(changed_args) => execute_changed(changed_args, options),
    }
}

fn execute_pack(args: PackArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let roots = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths
    };
    let max_items = options.limit.unwrap_or(200);

    let mut items = Vec::new();
    let mut file_count = 0usize;
    let mut directory_count = 0usize;
    let mut symbol_total = 0usize;
    let mut truncated = false;

    'roots: for root in &roots {
        let entries = enumerate_entries(root)?;
        for path in entries {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|source| AppError::file_metadata(path.clone(), source))?;
            let kind = if metadata.is_dir() {
                directory_count += 1;
                "directory".to_owned()
            } else if metadata.is_file() {
                file_count += 1;
                "file".to_owned()
            } else {
                "other".to_owned()
            };

            let (line_count, symbols) =
                if metadata.is_file() && is_text_candidate(&path, metadata.len()) {
                    let content = fs::read_to_string(&path)
                        .map_err(|source| AppError::file_read(path.clone(), source))?;
                    let line_count = content.lines().count();
                    let symbols = extract_symbols(&path, &content);
                    (line_count, symbols)
                } else {
                    (0usize, Vec::new())
                };
            symbol_total += symbols.len();

            items.push(PackItem {
                path: normalize_path(path.as_path()),
                kind,
                size_bytes: metadata.len(),
                line_count,
                symbol_count: symbols.len(),
                symbols: symbols.into_iter().take(8).collect(),
            });

            if items.len() >= max_items {
                truncated = true;
                break 'roots;
            }
        }
    }

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !items.is_empty() {
                println!(
                    "items: {} (files: {}, directories: {}, symbols: {})",
                    items.len(),
                    file_count,
                    directory_count,
                    symbol_total
                );
                for item in &items {
                    println!(
                        "{} | {} | size={} | lines={} | symbols={}",
                        item.kind, item.path, item.size_bytes, item.line_count, item.symbol_count
                    );
                    for symbol in &item.symbols {
                        println!("  - {}:{} {}", symbol.line, symbol.kind, symbol.name);
                    }
                }
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = CtxPackOutput {
                command: "ctx.pack",
                roots: roots
                    .iter()
                    .map(|path| normalize_path(path.as_path()))
                    .collect(),
                item_count: items.len(),
                file_count,
                directory_count,
                symbol_count: symbol_total,
                truncated,
                items,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_symbols(args: SymbolsArgs, options: &GlobalOptions) -> Result<(), AppError> {
    if !args.path.exists() {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            args.path.to_string_lossy()
        )));
    }

    let max_files = options.limit.unwrap_or(200);
    let candidate_files = enumerate_files_for_symbols(&args.path)?;
    let mut files = Vec::new();
    let mut symbol_total = 0usize;
    let mut truncated = false;

    for (index, path) in candidate_files.iter().enumerate() {
        if index >= max_files {
            truncated = true;
            break;
        }
        let metadata = fs::metadata(path)
            .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
        if !is_text_candidate(path, metadata.len()) {
            continue;
        }
        let content = fs::read_to_string(path)
            .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
        let symbols = extract_symbols(path, &content);
        symbol_total += symbols.len();
        if symbols.is_empty() {
            continue;
        }
        files.push(SymbolsFileOutput {
            path: normalize_path(path.as_path()),
            symbol_count: symbols.len(),
            symbols,
        });
    }

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            for file in &files {
                println!("{}", file.path);
                for symbol in &file.symbols {
                    println!("  {}:{} {}", symbol.line, symbol.kind, symbol.name);
                }
            }
            if truncated {
                eprintln!("warning: output truncated by --limit");
            }
        }
        OutputMode::Json => {
            let payload = CtxSymbolsOutput {
                command: "ctx.symbols",
                root: normalize_path(args.path.as_path()),
                file_count: files.len(),
                symbol_count: symbol_total,
                truncated,
                files,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn execute_changed(_args: ChangedArgs, options: &GlobalOptions) -> Result<(), AppError> {
    let in_repo = is_inside_git_repo()?;
    let entries = if in_repo {
        read_git_status_entries()?
    } else {
        Vec::new()
    };

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            if !in_repo {
                println!("not a git repository");
                return Ok(());
            }
            if entries.is_empty() {
                println!("working tree is clean");
                return Ok(());
            }
            for entry in &entries {
                match &entry.old_path {
                    Some(old_path) => println!("{} {} -> {}", entry.status, old_path, entry.path),
                    None => println!("{} {}", entry.status, entry.path),
                }
            }
        }
        OutputMode::Json => {
            let payload = CtxChangedOutput {
                command: "ctx.changed",
                in_git_repo: in_repo,
                changed_count: entries.len(),
                entries,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn enumerate_entries(root: &Path) -> Result<Vec<PathBuf>, AppError> {
    if !root.exists() {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            root.to_string_lossy()
        )));
    }
    if root.is_file() {
        return Ok(vec![root.to_path_buf()]);
    }

    let mut entries = Vec::new();
    for entry in WalkDir::new(root) {
        let entry = entry.map_err(|error| {
            AppError::directory_read(root.to_path_buf(), std::io::Error::other(error))
        })?;
        entries.push(entry.path().to_path_buf());
    }
    entries.sort();
    Ok(entries)
}

fn enumerate_files_for_symbols(path: &Path) -> Result<Vec<PathBuf>, AppError> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(path) {
        let entry = entry.map_err(|error| {
            AppError::directory_read(path.to_path_buf(), std::io::Error::other(error))
        })?;
        if entry.file_type().is_file() {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

fn is_text_candidate(path: &Path, size_bytes: u64) -> bool {
    if size_bytes > 2_000_000 {
        return false;
    }
    let Some(ext) = path.extension() else {
        return true;
    };
    let ext_lower = ext.to_string_lossy().to_lowercase();
    !matches!(
        ext_lower.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "ico"
            | "pdf"
            | "zip"
            | "7z"
            | "rar"
            | "exe"
            | "dll"
            | "bin"
            | "so"
            | "dylib"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "mp3"
            | "mp4"
            | "avi"
            | "mov"
    )
}

fn extract_symbols(path: &Path, content: &str) -> Vec<Symbol> {
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "rs" => extract_rust_symbols(content),
        "md" | "markdown" => extract_markdown_symbols(content),
        "py" => extract_python_symbols(content),
        "js" | "jsx" | "ts" | "tsx" | "vue" => extract_js_ts_symbols(content),
        "go" => extract_go_symbols(content),
        _ => extract_generic_symbols(content),
    }
}

fn extract_rust_symbols(content: &str) -> Vec<Symbol> {
    let fn_re =
        Regex::new(r"^\s*(pub\s+)?(async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let type_re = Regex::new(r"^\s*(pub\s+)?(struct|enum|trait)\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid regex");
    let impl_re =
        Regex::new(r"^\s*impl(\s*<[^>]+>)?\s+([A-Za-z_][A-Za-z0-9_:<>]*)").expect("valid regex");
    let mod_re = Regex::new(r"^\s*(pub\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");

    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = fn_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "fn".to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = type_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: captures[2].to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = impl_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "impl".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = mod_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "mod".to_owned(),
                name: captures[2].to_owned(),
            });
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
        if name.is_empty() {
            continue;
        }
        symbols.push(Symbol {
            line: index + 1,
            kind: format!("h{level}"),
            name: name.to_owned(),
        });
    }
    symbols
}

fn extract_python_symbols(content: &str) -> Vec<Symbol> {
    let class_re = Regex::new(r"^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let fn_re = Regex::new(r"^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = class_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "class".to_owned(),
                name: captures[1].to_owned(),
            });
            continue;
        }
        if let Some(captures) = fn_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "def".to_owned(),
                name: captures[1].to_owned(),
            });
        }
    }
    symbols
}

fn extract_js_ts_symbols(content: &str) -> Vec<Symbol> {
    let class_re =
        Regex::new(r"^\s*(export\s+)?class\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let function_re = Regex::new(r"^\s*(export\s+)?(async\s+)?function\s+([A-Za-z_][A-Za-z0-9_]*)")
        .expect("valid regex");
    let interface_re =
        Regex::new(r"^\s*(export\s+)?interface\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let type_re =
        Regex::new(r"^\s*(export\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)\s*=").expect("valid regex");
    let const_fn_re =
        Regex::new(r"^\s*(export\s+)?const\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(async\s*)?\(")
            .expect("valid regex");

    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = class_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "class".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = function_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "function".to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = interface_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "interface".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = type_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "type".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = const_fn_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "const-fn".to_owned(),
                name: captures[2].to_owned(),
            });
        }
    }
    symbols
}

fn extract_go_symbols(content: &str) -> Vec<Symbol> {
    let func_re = Regex::new(r"^\s*func\s+([A-Za-z_][A-Za-z0-9_]*)").expect("valid regex");
    let type_re = Regex::new(r"^\s*type\s+([A-Za-z_][A-Za-z0-9_]*)\s+").expect("valid regex");
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = func_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "func".to_owned(),
                name: captures[1].to_owned(),
            });
            continue;
        }
        if let Some(captures) = type_re.captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "type".to_owned(),
                name: captures[1].to_owned(),
            });
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
            symbols.push(Symbol {
                line: index + 1,
                kind: "symbol".to_owned(),
                name: trimmed.to_owned(),
            });
        }
    }
    symbols
}

fn is_inside_git_repo() -> Result<bool, AppError> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|error| AppError::invalid_argument(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        return Ok(false);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim() == "true")
}

fn read_git_status_entries() -> Result<Vec<ChangedEntry>, AppError> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .map_err(|error| {
            AppError::invalid_argument(format!("failed to run git status: {error}"))
        })?;
    if !output.status.success() {
        return Err(AppError::invalid_argument(
            "git status failed for current repository",
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut entries = Vec::new();

    for line in stdout.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = line[0..2].trim().to_owned();
        let rest = line[3..].to_owned();
        if let Some((old_path, new_path)) = rest.split_once(" -> ") {
            entries.push(ChangedEntry {
                status,
                path: normalize_slashes(new_path),
                old_path: Some(normalize_slashes(old_path)),
            });
        } else {
            entries.push(ChangedEntry {
                status,
                path: normalize_slashes(&rest),
                old_path: None,
            });
        }
    }

    Ok(entries)
}

fn normalize_slashes(value: &str) -> String {
    value.replace('\\', "/")
}

fn normalize_path(path: &Path) -> String {
    normalize_slashes(&path.to_string_lossy())
}
