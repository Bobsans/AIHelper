use std::path::Path;

use serde::Serialize;

use crate::commands::ctx_symbols::{extract_symbols, Symbol};
use crate::error::AppError;
use crate::safety::{TextFileDecision, TextFilePolicy, TextFileSkipReason};

use super::{adapters, ChangedArgs, PackArgs, SymbolsArgs};

#[derive(Debug, Serialize)]
pub(crate) struct SymbolsFileOutput {
    pub path: String,
    pub symbol_count: usize,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CtxSymbolsOutput {
    pub command: &'static str,
    pub preset: String,
    pub root: String,
    pub file_count: usize,
    pub symbol_count: usize,
    pub skipped_binary_files: usize,
    pub skipped_large_files: usize,
    pub skipped_symlink_files: usize,
    pub truncated: bool,
    pub files: Vec<SymbolsFileOutput>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PackItem {
    pub path: String,
    pub kind: String,
    pub size_bytes: u64,
    pub line_count: usize,
    pub symbol_count: usize,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CtxPackOutput {
    pub command: &'static str,
    pub preset: String,
    pub roots: Vec<String>,
    pub item_count: usize,
    pub file_count: usize,
    pub directory_count: usize,
    pub symbol_count: usize,
    pub skipped_binary_files: usize,
    pub skipped_large_files: usize,
    pub skipped_symlink_files: usize,
    pub truncated: bool,
    pub items: Vec<PackItem>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ChangedEntry {
    pub status: String,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CtxChangedOutput {
    pub command: &'static str,
    pub in_git_repo: bool,
    pub changed_count: usize,
    pub entries: Vec<ChangedEntry>,
}

#[derive(Debug)]
pub(crate) enum CtxResult {
    Pack(CtxPackOutput),
    Symbols(CtxSymbolsOutput),
    Changed(CtxChangedOutput),
}

#[derive(Default)]
struct SkipStats {
    binary_files: usize,
    large_files: usize,
    symlink_files: usize,
}

pub(crate) fn execute_pack(
    args: PackArgs,
    limit: Option<usize>,
) -> Result<CtxResult, AppError> {
    crate::safety::validate_max_bytes(args.max_bytes)?;
    let preset_settings = args.preset.settings();
    let roots = if args.paths.is_empty() {
        vec![Path::new(".").to_owned()]
    } else {
        args.paths
    };
    let max_items = limit.unwrap_or(preset_settings.default_limit);

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

        for entry in adapters::io::walk_entries(root, args.follow_symlinks)? {
            process_pack_entry(
                entry.path.as_path(),
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

    Ok(CtxResult::Pack(CtxPackOutput {
        command: "ctx.pack",
        preset: args.preset.as_str().to_owned(),
        roots: roots.iter().map(|path| adapters::io::normalize_path(path)).collect(),
        item_count: items.len(),
        file_count,
        directory_count,
        symbol_count: symbol_total,
        skipped_binary_files: skip_stats.binary_files,
        skipped_large_files: skip_stats.large_files,
        skipped_symlink_files: skip_stats.symlink_files,
        truncated,
        items,
    }))
}

#[allow(clippy::too_many_arguments)]
fn process_pack_entry(
    path: &Path,
    preset_settings: &super::PresetSettings,
    max_bytes: u64,
    follow_symlinks: bool,
    items: &mut Vec<PackItem>,
    file_count: &mut usize,
    directory_count: &mut usize,
    symbol_total: &mut usize,
    skip_stats: &mut SkipStats,
) -> Result<(), AppError> {
    let metadata = adapters::io::symlink_metadata(path)?;
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

    let (line_count, symbols) = match adapters::io::inspect_text_file(
        path,
        &TextFilePolicy {
            max_bytes,
            follow_symlinks,
        },
    )? {
        TextFileDecision::Allow(file_info) => {
            if !is_text_candidate(path, file_info.size_bytes) {
                (0usize, Vec::new())
            } else {
                let content = adapters::io::read_to_string(path)?;
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
        path: adapters::io::normalize_path(path),
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

pub(crate) fn execute_symbols(
    args: SymbolsArgs,
    limit: Option<usize>,
) -> Result<CtxResult, AppError> {
    crate::safety::validate_max_bytes(args.max_bytes)?;
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
    let max_files = limit.unwrap_or(preset_settings.default_limit);
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
        for entry in adapters::io::walk_entries(args.path.as_path(), args.follow_symlinks)? {
            if !entry.is_file {
                continue;
            }
            if scanned_files >= max_files {
                truncated = true;
                break;
            }
            scanned_files += 1;
            collect_symbols_for_file(
                entry.path.as_path(),
                &preset_settings,
                args.max_bytes,
                args.follow_symlinks,
                &mut files,
                &mut symbol_total,
                &mut skip_stats,
            )?;
        }
    }

    Ok(CtxResult::Symbols(CtxSymbolsOutput {
        command: "ctx.symbols",
        preset: args.preset.as_str().to_owned(),
        root: adapters::io::normalize_path(args.path.as_path()),
        file_count: files.len(),
        symbol_count: symbol_total,
        skipped_binary_files: skip_stats.binary_files,
        skipped_large_files: skip_stats.large_files,
        skipped_symlink_files: skip_stats.symlink_files,
        truncated,
        files,
    }))
}

fn collect_symbols_for_file(
    path: &Path,
    preset_settings: &super::PresetSettings,
    max_bytes: u64,
    follow_symlinks: bool,
    files: &mut Vec<SymbolsFileOutput>,
    symbol_total: &mut usize,
    skip_stats: &mut SkipStats,
) -> Result<(), AppError> {
    let inspect = adapters::io::inspect_text_file(
        path,
        &TextFilePolicy {
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

    let mut symbols = extract_symbols(path, &adapters::io::read_to_string(path)?);
    if symbols.len() > preset_settings.symbols_per_file_limit {
        symbols.truncate(preset_settings.symbols_per_file_limit);
    }
    *symbol_total += symbols.len();
    if symbols.is_empty() {
        return Ok(());
    }

    files.push(SymbolsFileOutput {
        path: adapters::io::normalize_path(path),
        symbol_count: symbols.len(),
        symbols,
    });

    Ok(())
}

pub(crate) fn execute_changed(_args: ChangedArgs) -> Result<CtxResult, AppError> {
    let in_repo = adapters::io::is_inside_git_repo()?;
    let entries = if in_repo {
        parse_changed_entries(adapters::io::read_git_status_lines()?)
    } else {
        Vec::new()
    };

    Ok(CtxResult::Changed(CtxChangedOutput {
        command: "ctx.changed",
        in_git_repo: in_repo,
        changed_count: entries.len(),
        entries,
    }))
}

fn parse_changed_entries(lines: Vec<String>) -> Vec<ChangedEntry> {
    let mut entries = Vec::new();

    for line in lines {
        if line.len() < 4 {
            continue;
        }
        let status = line[0..2].trim().to_owned();
        let rest = line[3..].to_owned();
        if let Some((old_path, new_path)) = rest.split_once(" -> ") {
            entries.push(ChangedEntry {
                status,
                path: normalize_path(&new_path),
                old_path: Some(normalize_path(&old_path)),
            });
        } else {
            entries.push(ChangedEntry {
                status,
                path: normalize_path(&rest),
                old_path: None,
            });
        }
    }

    entries
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
    Ok(adapters::io::symlink_metadata(path)?.file_type().is_symlink())
}

fn normalize_path(value: &str) -> String {
    adapters::io::normalize_path(Path::new(value))
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
