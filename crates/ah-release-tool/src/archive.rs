use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use ah_release_manifest::FilePurpose;
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

use crate::{RELEASE_PROFILES, ReleaseProfile, ReleaseToolError};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MANAGED_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub purpose: FilePurpose,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveInventory {
    pub profile: ReleaseProfile,
    pub archive_size: u64,
    pub archive_sha256: String,
    pub files: Vec<ArchiveFile>,
}

pub fn validate_archive_set(assets_dir: &Path) -> Result<Vec<ArchiveInventory>, ReleaseToolError> {
    let metadata = fs::symlink_metadata(assets_dir)
        .map_err(|source| ReleaseToolError::io("inspect assets directory", assets_dir, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ReleaseToolError::InvalidArchiveSet {
            detail: "assets path must be a real directory".to_owned(),
        });
    }

    let expected = RELEASE_PROFILES
        .iter()
        .map(|profile| profile.asset_name)
        .collect::<BTreeSet<_>>();
    let mut observed = BTreeSet::new();
    let entries = fs::read_dir(assets_dir)
        .map_err(|source| ReleaseToolError::io("read assets directory", assets_dir, source))?;
    for entry in entries {
        let entry = entry
            .map_err(|source| ReleaseToolError::io("read assets directory", assets_dir, source))?;
        let name =
            entry
                .file_name()
                .into_string()
                .map_err(|_| ReleaseToolError::InvalidArchiveSet {
                    detail: "asset filenames must be valid UTF-8".to_owned(),
                })?;
        let file_type = entry.file_type().map_err(|source| {
            ReleaseToolError::io("inspect release asset", entry.path(), source)
        })?;
        if file_type.is_symlink() || !file_type.is_file() {
            return Err(ReleaseToolError::InvalidArchiveSet {
                detail: format!("asset '{name}' must be a regular file"),
            });
        }
        observed.insert(name);
    }

    let observed_refs = observed.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if observed_refs != expected {
        return Err(ReleaseToolError::InvalidArchiveSet {
            detail: format!(
                "expected [{}], found [{}]",
                expected.iter().copied().collect::<Vec<_>>().join(", "),
                observed.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
        });
    }

    RELEASE_PROFILES
        .iter()
        .map(|profile| validate_archive(&assets_dir.join(profile.asset_name), *profile))
        .collect()
}

pub fn validate_archive(
    archive_path: &Path,
    profile: ReleaseProfile,
) -> Result<ArchiveInventory, ReleaseToolError> {
    let archive_metadata = fs::symlink_metadata(archive_path)
        .map_err(|source| ReleaseToolError::io("inspect release archive", archive_path, source))?;
    if archive_metadata.file_type().is_symlink() || !archive_metadata.is_file() {
        return Err(ReleaseToolError::archive(
            profile.asset_name,
            "archive path must be a regular file",
        ));
    }
    let archive_size = archive_metadata.len();
    if archive_size == 0 || archive_size > MAX_ARCHIVE_BYTES {
        return Err(ReleaseToolError::archive(
            profile.asset_name,
            format!("archive size {archive_size} is outside the allowed range"),
        ));
    }
    let archive_sha256 = hash_file(archive_path)?;

    let input = File::open(archive_path)
        .map_err(|source| ReleaseToolError::io("open release archive", archive_path, source))?;
    let mut archive = ZipArchive::new(input).map_err(|error| {
        ReleaseToolError::archive(profile.asset_name, format!("malformed ZIP: {error}"))
    })?;
    let expected = profile
        .managed_paths()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let mut observed_paths = BTreeSet::new();
    let mut equivalent_paths = BTreeSet::new();
    let mut files = Vec::with_capacity(expected.len());
    let mut total_uncompressed = 0_u64;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            ReleaseToolError::archive(
                profile.asset_name,
                format!("cannot read ZIP entry {index}: {error}"),
            )
        })?;
        let name = entry.name().to_owned();
        validate_entry_name(profile.asset_name, &name, entry.is_dir())?;
        if entry.encrypted() {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                name,
                "encrypted entries are not allowed",
            ));
        }
        if entry.is_symlink() {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                name,
                "symbolic links are not allowed",
            ));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                name,
                "unsupported compression method",
            ));
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| !supported_unix_mode(mode, entry.is_dir()))
        {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                name,
                "unsupported Unix file type",
            ));
        }

        let normalized = name.trim_end_matches('/').to_owned();
        let equivalent = normalized.to_ascii_lowercase();
        if !equivalent_paths.insert(equivalent) {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                normalized,
                "duplicate or case-colliding path",
            ));
        }
        if entry.is_dir() {
            if normalized != "plugins" {
                return Err(ReleaseToolError::entry(
                    profile.asset_name,
                    normalized,
                    "unexpected directory entry",
                ));
            }
            continue;
        }
        if !observed_paths.insert(normalized.clone()) {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                normalized,
                "duplicate file path",
            ));
        }
        let Some(purpose) = expected.get(&normalized).copied() else {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                normalized,
                "file is not part of the release profile",
            ));
        };
        if entry.size() > MAX_MANAGED_FILE_BYTES {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                normalized,
                format!("file size {} exceeds the allowed maximum", entry.size()),
            ));
        }
        total_uncompressed = total_uncompressed
            .checked_add(entry.size())
            .ok_or_else(|| {
                ReleaseToolError::archive(profile.asset_name, "uncompressed size overflow")
            })?;
        if total_uncompressed > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(ReleaseToolError::archive(
                profile.asset_name,
                "total uncompressed size exceeds the allowed maximum",
            ));
        }
        let (size, sha256) = hash_reader(&mut entry, MAX_MANAGED_FILE_BYTES).map_err(|error| {
            ReleaseToolError::entry(
                profile.asset_name,
                normalized.clone(),
                format!("cannot hash file contents: {error}"),
            )
        })?;
        if size != entry.size() {
            return Err(ReleaseToolError::entry(
                profile.asset_name,
                normalized,
                format!(
                    "declared size {} does not match {size} bytes read",
                    entry.size()
                ),
            ));
        }
        files.push(ArchiveFile {
            path: normalized,
            size,
            sha256,
            purpose,
        });
    }

    let expected_paths = expected.keys().cloned().collect::<BTreeSet<_>>();
    if observed_paths != expected_paths {
        let missing = expected_paths
            .difference(&observed_paths)
            .cloned()
            .collect::<Vec<_>>();
        return Err(ReleaseToolError::archive(
            profile.asset_name,
            format!("missing required files: {}", missing.join(", ")),
        ));
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(ArchiveInventory {
        profile,
        archive_size,
        archive_sha256,
        files,
    })
}

fn supported_unix_mode(mode: u32, is_directory: bool) -> bool {
    let file_type = mode & 0o170000;
    if is_directory {
        matches!(file_type, 0 | 0o040000)
    } else {
        matches!(file_type, 0 | 0o100000)
    }
}

fn validate_entry_name(
    archive: &str,
    name: &str,
    is_directory: bool,
) -> Result<(), ReleaseToolError> {
    if name.is_empty() || !name.is_ascii() || name.contains(['\\', '\0', ':']) {
        return Err(ReleaseToolError::entry(
            archive,
            printable_entry(name),
            "path must be non-empty ASCII using '/' separators",
        ));
    }
    if name.starts_with('/') || (is_directory && !name.ends_with('/')) {
        return Err(ReleaseToolError::entry(
            archive,
            printable_entry(name),
            "path must be relative and directory entries must end with '/'",
        ));
    }
    let logical = name.trim_end_matches('/');
    if logical.is_empty()
        || logical
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ReleaseToolError::entry(
            archive,
            printable_entry(name),
            "path contains an unsafe component",
        ));
    }
    Ok(())
}

fn printable_entry(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_graphic() || character == ' ' {
                character
            } else {
                '?'
            }
        })
        .collect()
}

fn hash_file(path: &Path) -> Result<String, ReleaseToolError> {
    let mut input = File::open(path)
        .map_err(|source| ReleaseToolError::io("open release archive", path, source))?;
    let (_, digest) = hash_reader(&mut input, MAX_ARCHIVE_BYTES)
        .map_err(|source| ReleaseToolError::io("hash release archive", path, source))?;
    Ok(digest)
}

fn hash_reader(reader: &mut impl Read, maximum: u64) -> io::Result<(u64, String)> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("input size overflow"))?;
        if total > maximum {
            return Err(io::Error::other("input exceeds the allowed maximum"));
        }
        digest.update(&buffer[..read]);
    }
    let bytes = digest.finalize();
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok((total, encoded))
}

#[cfg(test)]
mod tests {
    use super::supported_unix_mode;

    #[test]
    fn unix_modes_accept_only_regular_files_and_directories() {
        assert!(supported_unix_mode(0, false));
        assert!(supported_unix_mode(0o100755, false));
        assert!(supported_unix_mode(0, true));
        assert!(supported_unix_mode(0o040755, true));
        assert!(!supported_unix_mode(0o010644, false));
        assert!(!supported_unix_mode(0o120777, false));
        assert!(!supported_unix_mode(0o100755, true));
    }
}
