use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use clap::{Args, Subcommand, ValueEnum};
use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

use crate::{
    cli::GlobalOptions,
    error::AppError,
    output::OutputMode,
    safety::{self, TextFileDecision, TextFilePolicy, TextFileSkipReason},
};

#[derive(Debug, Args)]
pub struct CtxArgs {
    #[command(subcommand)]
    pub command: CtxCommand,
}

#[derive(Debug, Subcommand)]
pub enum CtxCommand {
    #[command(about = "Pack files/directories into compact context metadata")]
    Pack(PackArgs),
    #[command(about = "Extract symbols from file(s)")]
    Symbols(SymbolsArgs),
    #[command(about = "Show changed paths from git status")]
    Changed(ChangedArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CtxPreset {
    Summary,
    Review,
    Debug,
}

#[derive(Debug, Args)]
pub struct PackArgs {
    pub paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    pub preset: CtxPreset,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
}

#[derive(Debug, Args)]
pub struct SymbolsArgs {
    pub path: PathBuf,
    #[arg(long, value_enum, default_value_t = CtxPreset::Review)]
    pub preset: CtxPreset,
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = safety::DEFAULT_MAX_TEXT_BYTES,
        help = "Skip files larger than this size while extracting symbols"
    )]
    pub max_bytes: u64,
    #[arg(long, help = "Follow symlink directories during traversal")]
    pub follow_symlinks: bool,
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
    preset: String,
    root: String,
    file_count: usize,
    symbol_count: usize,
    skipped_binary_files: usize,
    skipped_large_files: usize,
    skipped_symlink_files: usize,
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
    preset: String,
    roots: Vec<String>,
    item_count: usize,
    file_count: usize,
    directory_count: usize,
    symbol_count: usize,
    skipped_binary_files: usize,
    skipped_large_files: usize,
    skipped_symlink_files: usize,
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

#[derive(Debug, Clone, Copy)]
struct PresetSettings {
    default_limit: usize,
    pack_symbol_preview_limit: usize,
    symbols_per_file_limit: usize,
}

#[derive(Default)]
struct SkipStats {
    binary_files: usize,
    large_files: usize,
    symlink_files: usize,
}

impl CtxPreset {
    fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Review => "review",
            Self::Debug => "debug",
        }
    }

    fn settings(self) -> PresetSettings {
        match self {
            Self::Summary => PresetSettings {
                default_limit: 80,
                pack_symbol_preview_limit: 4,
                symbols_per_file_limit: 20,
            },
            Self::Review => PresetSettings {
                default_limit: 200,
                pack_symbol_preview_limit: 8,
                symbols_per_file_limit: 80,
            },
            Self::Debug => PresetSettings {
                default_limit: 500,
                pack_symbol_preview_limit: 16,
                symbols_per_file_limit: 200,
            },
        }
    }
}

pub fn execute(args: CtxArgs, options: &GlobalOptions) -> Result<(), AppError> {
    match args.command {
        CtxCommand::Pack(pack_args) => execute_pack(pack_args, options),
        CtxCommand::Symbols(symbols_args) => execute_symbols(symbols_args, options),
        CtxCommand::Changed(changed_args) => execute_changed(changed_args, options),
    }
}

fn execute_pack(args: PackArgs, options: &GlobalOptions) -> Result<(), AppError> {
    safety::validate_max_bytes(args.max_bytes)?;
    let preset_settings = args.preset.settings();
    let roots = if args.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        args.paths
    };
    let max_items = options.limit.unwrap_or(preset_settings.default_limit);

    let mut items = Vec::new();
    let mut file_count = 0usize;
    let mut directory_count = 0usize;
    let mut symbol_total = 0usize;
    let mut skip_stats = SkipStats::default();
    let mut truncated = false;

    'roots: for root in &roots {
        if !root.exists() {
            return Err(AppError::invalid_argument(format!(
                "path does not exist: {}",
                root.to_string_lossy()
            )));
        }

        if root.is_file() {
            process_pack_entry(
                root,
                &preset_settings,
                args.max_bytes,
                args.follow_symlinks,
                &mut items,
                &mut file_count,
                &mut directory_count,
                &mut symbol_total,
                &mut skip_stats,
            )?;
            if items.len() >= max_items {
                truncated = true;
                break 'roots;
            }
            continue;
        }

        for entry in WalkDir::new(root)
            .follow_links(args.follow_symlinks)
            .sort_by_file_name()
        {
            let entry = match entry {
                Ok(value) => value,
                Err(error) if error.loop_ancestor().is_some() => continue,
                Err(error) => {
                    return Err(AppError::directory_read(
                        root.to_path_buf(),
                        std::io::Error::other(error),
                    ));
                }
            };
            process_pack_entry(
                entry.path(),
                &preset_settings,
                args.max_bytes,
                args.follow_symlinks,
                &mut items,
                &mut file_count,
                &mut directory_count,
                &mut symbol_total,
                &mut skip_stats,
            )?;
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
                println!("preset: {}", args.preset.as_str());
                println!(
                    "items: {} (files: {}, directories: {}, symbols: {})",
                    items.len(),
                    file_count,
                    directory_count,
                    symbol_total
                );
                println!(
                    "skipped: binary={} large={} symlink={}",
                    skip_stats.binary_files, skip_stats.large_files, skip_stats.symlink_files
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
                preset: args.preset.as_str().to_owned(),
                roots: roots
                    .iter()
                    .map(|path| normalize_path(path.as_path()))
                    .collect(),
                item_count: items.len(),
                file_count,
                directory_count,
                symbol_count: symbol_total,
                skipped_binary_files: skip_stats.binary_files,
                skipped_large_files: skip_stats.large_files,
                skipped_symlink_files: skip_stats.symlink_files,
                truncated,
                items,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn process_pack_entry(
    path: &Path,
    preset_settings: &PresetSettings,
    max_bytes: u64,
    follow_symlinks: bool,
    items: &mut Vec<PackItem>,
    file_count: &mut usize,
    directory_count: &mut usize,
    symbol_total: &mut usize,
    skip_stats: &mut SkipStats,
) -> Result<(), AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    let kind = if metadata.file_type().is_symlink() {
        "symlink".to_owned()
    } else if metadata.is_dir() {
        *directory_count += 1;
        "directory".to_owned()
    } else if metadata.is_file() {
        *file_count += 1;
        "file".to_owned()
    } else {
        "other".to_owned()
    };

    let (line_count, symbols) = match safety::inspect_text_file(
        path,
        TextFilePolicy {
            max_bytes,
            follow_symlinks,
        },
    )? {
        TextFileDecision::Allow(file_info) => {
            if !is_text_candidate(path, file_info.size_bytes) {
                (0usize, Vec::new())
            } else {
                let content = fs::read_to_string(path)
                    .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
                let line_count = content.lines().count();
                let symbols = extract_symbols(path, &content);
                (line_count, symbols)
            }
        }
        TextFileDecision::Skip(reason) => {
            register_skip_reason(skip_stats, reason);
            (0usize, Vec::new())
        }
    };
    *symbol_total += symbols.len();

    items.push(PackItem {
        path: normalize_path(path),
        kind,
        size_bytes: metadata.len(),
        line_count,
        symbol_count: symbols.len(),
        symbols: symbols
            .into_iter()
            .take(preset_settings.pack_symbol_preview_limit)
            .collect(),
    });

    Ok(())
}

fn execute_symbols(args: SymbolsArgs, options: &GlobalOptions) -> Result<(), AppError> {
    safety::validate_max_bytes(args.max_bytes)?;
    if !args.path.exists() {
        return Err(AppError::invalid_argument(format!(
            "path does not exist: {}",
            args.path.to_string_lossy()
        )));
    }
    if !args.follow_symlinks && is_symlink_path(args.path.as_path())? {
        return Err(AppError::invalid_argument(format!(
            "path is a symlink and symlink traversal is disabled: {} (use --follow-symlinks)",
            args.path.to_string_lossy()
        )));
    }

    let preset_settings = args.preset.settings();
    let max_files = options.limit.unwrap_or(preset_settings.default_limit);
    let mut files = Vec::new();
    let mut symbol_total = 0usize;
    let mut skip_stats = SkipStats::default();
    let mut truncated = false;
    let mut scanned_files = 0usize;

    if args.path.is_file() {
        collect_symbols_for_file(
            args.path.as_path(),
            &preset_settings,
            args.max_bytes,
            args.follow_symlinks,
            &mut files,
            &mut symbol_total,
            &mut skip_stats,
        )?;
    } else {
        for entry in WalkDir::new(&args.path)
            .follow_links(args.follow_symlinks)
            .sort_by_file_name()
        {
            let entry = match entry {
                Ok(value) => value,
                Err(error) if error.loop_ancestor().is_some() => continue,
                Err(error) => {
                    return Err(AppError::directory_read(
                        args.path.to_path_buf(),
                        std::io::Error::other(error),
                    ));
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            if scanned_files >= max_files {
                truncated = true;
                break;
            }
            scanned_files += 1;
            collect_symbols_for_file(
                entry.path(),
                &preset_settings,
                args.max_bytes,
                args.follow_symlinks,
                &mut files,
                &mut symbol_total,
                &mut skip_stats,
            )?;
        }
    }

    if options.quiet {
        return Ok(());
    }

    match options.output {
        OutputMode::Text => {
            println!("preset: {}", args.preset.as_str());
            println!(
                "skipped: binary={} large={} symlink={}",
                skip_stats.binary_files, skip_stats.large_files, skip_stats.symlink_files
            );
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
                preset: args.preset.as_str().to_owned(),
                root: normalize_path(args.path.as_path()),
                file_count: files.len(),
                symbol_count: symbol_total,
                skipped_binary_files: skip_stats.binary_files,
                skipped_large_files: skip_stats.large_files,
                skipped_symlink_files: skip_stats.symlink_files,
                truncated,
                files,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }

    Ok(())
}

fn collect_symbols_for_file(
    path: &Path,
    preset_settings: &PresetSettings,
    max_bytes: u64,
    follow_symlinks: bool,
    files: &mut Vec<SymbolsFileOutput>,
    symbol_total: &mut usize,
    skip_stats: &mut SkipStats,
) -> Result<(), AppError> {
    let inspect = safety::inspect_text_file(
        path,
        TextFilePolicy {
            max_bytes,
            follow_symlinks,
        },
    )?;
    let file_info = match inspect {
        TextFileDecision::Allow(value) => value,
        TextFileDecision::Skip(reason) => {
            register_skip_reason(skip_stats, reason);
            return Ok(());
        }
    };
    if !is_text_candidate(path, file_info.size_bytes) {
        return Ok(());
    }

    let content = fs::read_to_string(path)
        .map_err(|source| AppError::file_read(path.to_path_buf(), source))?;
    let mut symbols = extract_symbols(path, &content);
    if symbols.len() > preset_settings.symbols_per_file_limit {
        symbols.truncate(preset_settings.symbols_per_file_limit);
    }
    *symbol_total += symbols.len();
    if symbols.is_empty() {
        return Ok(());
    }

    files.push(SymbolsFileOutput {
        path: normalize_path(path),
        symbol_count: symbols.len(),
        symbols,
    });

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

fn register_skip_reason(skip_stats: &mut SkipStats, reason: TextFileSkipReason) {
    match reason {
        TextFileSkipReason::Binary => {
            skip_stats.binary_files += 1;
        }
        TextFileSkipReason::TooLarge { .. } => {
            skip_stats.large_files += 1;
        }
        TextFileSkipReason::SymlinkBlocked => {
            skip_stats.symlink_files += 1;
        }
        TextFileSkipReason::NotAFile => {}
    }
}

fn is_symlink_path(path: &Path) -> Result<bool, AppError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| AppError::file_metadata(path.to_path_buf(), source))?;
    Ok(metadata.file_type().is_symlink())
}

fn is_text_candidate(path: &Path, _size_bytes: u64) -> bool {
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
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = rust_fn_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "fn".to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = rust_type_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: captures[2].to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = rust_impl_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "impl".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = rust_mod_re().captures(line) {
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
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = python_class_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "class".to_owned(),
                name: captures[1].to_owned(),
            });
            continue;
        }
        if let Some(captures) = python_fn_re().captures(line) {
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
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = js_class_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "class".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = js_function_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "function".to_owned(),
                name: captures[3].to_owned(),
            });
            continue;
        }
        if let Some(captures) = js_interface_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "interface".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = js_type_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "type".to_owned(),
                name: captures[2].to_owned(),
            });
            continue;
        }
        if let Some(captures) = js_const_fn_re().captures(line) {
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
    let mut symbols = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(captures) = go_func_re().captures(line) {
            symbols.push(Symbol {
                line: index + 1,
                kind: "func".to_owned(),
                name: captures[1].to_owned(),
            });
            continue;
        }
        if let Some(captures) = go_type_re().captures(line) {
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
    RE.get_or_init(|| compile_regex(r"^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)"))
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

fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid ctx symbol regex")
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
