//! Where a transaction's files live, and the checks that a caller handed us a
//! usable set of roots.
//!
//! The bounds live here too: they describe the on-disk shape as much as the
//! directory names do.

use std::{
    fs::{self},
    io::{self},
    path::{Component, Path, PathBuf},
};

use ah_updater_core::{TransactionPlanV1, UpdaterError};

use super::fsverify::{create_synced_directory, ensure_direct_directory, paths_equal};
use super::{recovery, transaction};

pub(super) const IDENTITY_FILE: &str = "identity.json";

pub(super) const INSTALLED_MANIFEST_FILE: &str = "installed.manifest.json";

pub(super) const INSTALLED_SIGNATURE_FILE: &str = "installed.manifest.sig";

pub(super) const PLAN_FILE: &str = "plan.json";

pub(super) const JOURNAL_FILE: &str = "journal.json";

pub(super) const CANDIDATE_DIRECTORY: &str = "candidate";

pub(super) const BACKUP_DIRECTORY: &str = "backup";

pub(super) const FILES_DIRECTORY: &str = "files";

pub(super) const PERMANENT_BACKUP_DIRECTORY: &str = "backup";

pub(super) const MAX_IDENTITY_BYTES: usize = 16 * 1024;

pub(super) const MAX_PLAN_BYTES: usize = 4 * 1024 * 1024;

pub(super) const MAX_JOURNAL_BYTES: usize = 64 * 1024;

pub(super) const MAX_MANAGED_FILE_BYTES: u64 = 256 * 1024 * 1024;

pub(super) const MAX_TOTAL_MANAGED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionPaths {
    pub(super) installation_root: PathBuf,
    pub(super) installation_state_root: PathBuf,
    pub(super) transaction_root: PathBuf,
}

pub(super) fn validate_transaction_location(
    paths: &TransactionPaths,
    plan: &TransactionPlanV1,
) -> Result<(), UpdaterError> {
    let expected = paths
        .installation_state_root
        .join("transactions")
        .join(plan.transaction_id.to_string());
    if !paths_equal(&paths.transaction_root, &expected) {
        return Err(recovery(
            "durable transaction root is not bound to its installation state",
        ));
    }
    Ok(())
}

pub(super) fn validate_prepare_paths(
    paths: &TransactionPaths,
    candidate_source_root: &Path,
) -> Result<(), UpdaterError> {
    validate_absolute_paths(paths)?;
    if !candidate_source_root.is_absolute() {
        return Err(transaction("candidate source root must be absolute"));
    }
    ensure_direct_directory(&paths.installation_root)?;
    ensure_direct_directory(&paths.installation_state_root)?;
    ensure_direct_directory(candidate_source_root)?;
    let transaction_parent = paths
        .transaction_root
        .parent()
        .ok_or_else(|| transaction("transaction root has no parent directory"))?;
    ensure_direct_directory(transaction_parent)?;
    match fs::symlink_metadata(&paths.transaction_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(transaction("transaction root already exists")),
        Err(_) => return Err(transaction("failed to inspect transaction root")),
    }

    let installation = fs::canonicalize(&paths.installation_root)
        .map_err(|_| transaction("failed to resolve the installation root"))?;
    let state = fs::canonicalize(&paths.installation_state_root)
        .map_err(|_| transaction("failed to resolve the installation state root"))?;
    let candidate = fs::canonicalize(candidate_source_root)
        .map_err(|_| transaction("failed to resolve the candidate source root"))?;
    let parent = fs::canonicalize(transaction_parent)
        .map_err(|_| transaction("failed to resolve the transaction parent"))?;
    let name = paths
        .transaction_root
        .file_name()
        .ok_or_else(|| transaction("transaction root has no directory name"))?;
    let transaction_root = parent.join(name);
    if overlaps(&installation, &state)
        || overlaps(&installation, &candidate)
        || overlaps(&installation, &transaction_root)
        || overlaps(&candidate, &transaction_root)
    {
        return Err(transaction(
            "installation, state, candidate, and transaction roots overlap unsafely",
        ));
    }
    Ok(())
}

pub(super) fn validate_existing_paths(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    validate_absolute_paths(paths)?;
    ensure_direct_directory(&paths.installation_root)?;
    ensure_direct_directory(&paths.installation_state_root)?;
    ensure_direct_directory(&paths.transaction_root)?;
    ensure_direct_directory(&paths.candidate_root())?;
    ensure_direct_directory(&paths.candidate_files_root())?;
    ensure_direct_directory(&paths.backup_root())?;
    ensure_direct_directory(&paths.backup_files_root())?;
    let installation = fs::canonicalize(&paths.installation_root)
        .map_err(|_| recovery("failed to resolve the installation root"))?;
    let state = fs::canonicalize(&paths.installation_state_root)
        .map_err(|_| recovery("failed to resolve the installation state root"))?;
    let transaction_root = fs::canonicalize(&paths.transaction_root)
        .map_err(|_| recovery("failed to resolve the transaction root"))?;
    if overlaps(&installation, &state) || overlaps(&installation, &transaction_root) {
        return Err(recovery(
            "installation and durable transaction state roots overlap",
        ));
    }
    Ok(())
}

pub(super) fn validate_absolute_paths(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    for path in [
        &paths.installation_root,
        &paths.installation_state_root,
        &paths.transaction_root,
    ] {
        if !path.is_absolute()
            || path.to_str().is_none()
            || path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(transaction(
                "transaction paths must be absolute normalized Unicode paths",
            ));
        }
    }
    Ok(())
}

pub(super) fn overlaps(left: &Path, right: &Path) -> bool {
    path_starts_with(left, right) || path_starts_with(right, left)
}

#[cfg(windows)]
pub(super) fn path_starts_with(path: &Path, base: &Path) -> bool {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let base = base.to_string_lossy().replace('/', "\\").to_lowercase();
    path == base
        || path
            .strip_prefix(&base)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

#[cfg(not(windows))]
pub(super) fn path_starts_with(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

pub(super) fn create_transaction_layout(paths: &TransactionPaths) -> Result<(), UpdaterError> {
    create_synced_directory(&paths.transaction_root)?;
    create_synced_directory(&paths.candidate_root())?;
    create_synced_directory(&paths.candidate_files_root())?;
    create_synced_directory(&paths.backup_root())?;
    create_synced_directory(&paths.backup_files_root())
}
