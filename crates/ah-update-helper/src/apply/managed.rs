//! Mutating the managed files themselves: staging a copy, verifying it, and
//! replacing the live file with it.
//!
//! Each operation stages to a temporary beside its destination and replaces
//! atomically, so an interrupted step leaves either the old file or the new
//! one - never a partial write.

use std::{
    fs::{self, File, OpenOptions},
    io::{self},
    path::{Path, PathBuf},
};

use ah_updater_core::{ManagedFile, ManagedFileOperationV1, ReleaseManifest, UpdaterError};

use super::fsverify::{
    atomic_replace, copy_exact_hash, create_managed_destination, ensure_direct_file,
    existing_managed_file, file_matches, prospective_managed_file, remove_file_synced, sync_parent,
    verify_file,
};
use super::journal::mutate_with_checkpoint;
use super::layout::{MAX_MANAGED_FILE_BYTES, MAX_TOTAL_MANAGED_BYTES};
use super::{
    FailureInjector, LoadedTransaction, TransactionRunError, activation, recovery, rollback,
    transaction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMutationPhase {
    StageCandidate,
    BackupManaged,
    ActivateManaged,
    PublishInstalledRecord,
    RollbackManaged,
    RestoreInstalledRecord,
}

pub(super) fn apply_activation_operation(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    index: usize,
) -> Result<(), UpdaterError> {
    let root = &transaction.paths.installation_root;
    match operation {
        ManagedFileOperationV1::Add { new } => {
            let destination = optional_managed_file(root, &new.path)?;
            if let Some(destination) = destination {
                if file_matches(&destination, new)? {
                    return Ok(());
                }
                return Err(activation(
                    "transaction add destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.candidate_files_root(),
                new,
                FileMutationPhase::ActivateManaged,
                index,
            )
        }
        ManagedFileOperationV1::Replace { old, new } => {
            let destination = optional_managed_file(root, &new.path)?
                .ok_or_else(|| activation("transaction replacement destination is missing"))?;
            if file_matches(&destination, new)? {
                return Ok(());
            }
            if !file_matches(&destination, old)? {
                return Err(activation(
                    "transaction replacement destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.candidate_files_root(),
                new,
                FileMutationPhase::ActivateManaged,
                index,
            )
        }
        ManagedFileOperationV1::Remove { old } => {
            let Some(destination) = optional_managed_file(root, &old.path)? else {
                return Ok(());
            };
            if !file_matches(&destination, old)? {
                return Err(activation(
                    "transaction removal destination contains unknown file content",
                ));
            }
            remove_file_synced(&destination)
                .map_err(|_| activation("failed to remove obsolete managed file"))
        }
    }
}

pub(super) fn apply_rollback_operation(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    index: usize,
) -> Result<(), UpdaterError> {
    remove_managed_operation_temp(
        transaction,
        operation,
        FileMutationPhase::ActivateManaged,
        index,
    )?;
    let root = &transaction.paths.installation_root;
    match operation {
        ManagedFileOperationV1::Add { new } => {
            let Some(destination) = optional_managed_file(root, &new.path)? else {
                return Ok(());
            };
            if file_matches(&destination, new)? {
                remove_file_synced(&destination)
                    .map_err(|_| rollback("failed to remove added managed file during rollback"))?;
            }
            Ok(())
        }
        ManagedFileOperationV1::Replace { old, new } => {
            if let Some(destination) = optional_managed_file(root, &old.path)? {
                if file_matches(&destination, old)? {
                    return Ok(());
                }
                if !file_matches(&destination, new)? {
                    return Err(rollback(
                        "rollback replacement destination contains unknown file content",
                    ));
                }
            }
            install_managed_file(
                transaction,
                &transaction.paths.backup_files_root(),
                old,
                FileMutationPhase::RollbackManaged,
                index,
            )
        }
        ManagedFileOperationV1::Remove { old } => {
            if let Some(destination) = optional_managed_file(root, &old.path)? {
                if file_matches(&destination, old)? {
                    return Ok(());
                }
                return Err(rollback(
                    "rollback removal destination contains unknown file content",
                ));
            }
            install_managed_file(
                transaction,
                &transaction.paths.backup_files_root(),
                old,
                FileMutationPhase::RollbackManaged,
                index,
            )
        }
    }
}

pub(super) fn install_managed_file(
    loaded: &LoadedTransaction,
    source_root: &Path,
    expected: &ManagedFile,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let source = existing_managed_file(source_root, &expected.path)?;
    verify_file(&source, expected)?;
    let destination = create_managed_destination(&loaded.paths.installation_root, &expected.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| transaction("managed destination has no parent directory"))?;
    let temporary = parent.join(operation_temp_name(loaded, phase, index));
    prepare_managed_temp(&source, &temporary, expected)?;
    atomic_replace(&temporary, &destination)
        .map_err(|_| mutation_error(phase, "failed to atomically replace managed file"))?;
    verify_file(&destination, expected)
        .map_err(|_| mutation_error(phase, "replaced managed file failed verification"))
}

pub(super) fn prepare_managed_temp(
    source: &Path,
    temporary: &Path,
    expected: &ManagedFile,
) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            copy_verified_file(source, temporary, expected)
        }
        Ok(_) => {
            ensure_direct_file(temporary)?;
            if !file_matches(temporary, expected)? {
                return Err(recovery(
                    "managed operation temporary file contains unknown content",
                ));
            }
            Ok(())
        }
        Err(_) => Err(recovery(
            "failed to inspect managed operation temporary file",
        )),
    }
}

pub(super) fn remove_managed_operation_temp(
    transaction: &LoadedTransaction,
    operation: &ManagedFileOperationV1,
    phase: FileMutationPhase,
    index: usize,
) -> Result<(), UpdaterError> {
    let expected = match operation {
        ManagedFileOperationV1::Add { new } | ManagedFileOperationV1::Replace { new, .. } => {
            Some(new)
        }
        ManagedFileOperationV1::Remove { .. } => None,
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    let destination =
        prospective_managed_file(&transaction.paths.installation_root, &expected.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| recovery("managed destination has no parent directory"))?;
    let temporary = parent.join(operation_temp_name(transaction, phase, index));
    remove_known_managed_temp(&temporary, expected)
}

pub(super) fn remove_known_managed_temp(
    temporary: &Path,
    expected: &ManagedFile,
) -> Result<(), UpdaterError> {
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => {
            ensure_direct_file(temporary)?;
            if !file_matches(temporary, expected)? {
                return Err(recovery(
                    "managed operation temporary file contains unknown content",
                ));
            }
            remove_file_synced(temporary)
                .map_err(|_| recovery("failed to remove managed operation temporary file"))
        }
        Err(_) => Err(recovery(
            "failed to inspect managed operation temporary file",
        )),
    }
}

pub(super) fn operation_temp_name(
    transaction: &LoadedTransaction,
    phase: FileMutationPhase,
    index: usize,
) -> String {
    format!(
        ".ah-update-{}-{}-{index}.tmp",
        transaction.plan.transaction_id,
        phase_name(phase)
    )
}

pub(super) fn phase_name(phase: FileMutationPhase) -> &'static str {
    match phase {
        FileMutationPhase::StageCandidate => "stage",
        FileMutationPhase::BackupManaged => "backup",
        FileMutationPhase::ActivateManaged => "activate",
        FileMutationPhase::PublishInstalledRecord => "publish",
        FileMutationPhase::RollbackManaged => "rollback",
        FileMutationPhase::RestoreInstalledRecord => "restore",
    }
}

pub(super) fn mutation_error(phase: FileMutationPhase, detail: &'static str) -> UpdaterError {
    match phase {
        FileMutationPhase::StageCandidate | FileMutationPhase::BackupManaged => transaction(detail),
        FileMutationPhase::ActivateManaged | FileMutationPhase::PublishInstalledRecord => {
            activation(detail)
        }
        FileMutationPhase::RollbackManaged | FileMutationPhase::RestoreInstalledRecord => {
            rollback(detail)
        }
    }
}

pub(super) fn optional_managed_file(
    root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, UpdaterError> {
    let path = prospective_managed_file(root, relative)?;
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Ok(_) => {
            ensure_direct_file(&path)?;
            Ok(Some(path))
        }
        Err(_) => Err(transaction("failed to inspect managed destination file")),
    }
}

pub(super) fn copy_manifest_files_with_injector(
    injector: &mut impl FailureInjector,
    phase: FileMutationPhase,
    source_root: &Path,
    destination_root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), TransactionRunError> {
    validate_inventory_bounds(manifest)?;
    for (index, managed) in manifest.files.iter().enumerate() {
        mutate_with_checkpoint(injector, phase, index, &managed.path, || {
            let source = existing_managed_file(source_root, &managed.path)?;
            let destination = create_managed_destination(destination_root, &managed.path)?;
            copy_verified_file(&source, &destination, managed)
        })?;
    }
    Ok(())
}

pub(super) fn verify_manifest_files(
    root: &Path,
    manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    validate_inventory_bounds(manifest)?;
    for managed in &manifest.files {
        let path = existing_managed_file(root, &managed.path)?;
        verify_file(&path, managed)?;
    }
    Ok(())
}

pub(super) fn validate_inventory_bounds(manifest: &ReleaseManifest) -> Result<(), UpdaterError> {
    manifest
        .files
        .iter()
        .try_fold(0_u64, |total, file| {
            if file.size > MAX_MANAGED_FILE_BYTES {
                return Err(transaction(
                    "managed file exceeds the transaction size limit",
                ));
            }
            total
                .checked_add(file.size)
                .filter(|total| *total <= MAX_TOTAL_MANAGED_BYTES)
                .ok_or_else(|| transaction("managed files exceed the total transaction size limit"))
        })
        .map(|_| ())
}

pub(super) fn verify_add_destinations_absent(
    installation_root: &Path,
    operations: &[ManagedFileOperationV1],
    new_manifest: &ReleaseManifest,
) -> Result<(), UpdaterError> {
    for operation in operations {
        if let ManagedFileOperationV1::Add { new } = operation {
            if !new_manifest.files.iter().any(|file| file == new) {
                return Err(transaction(
                    "transaction add operation is absent from the new manifest",
                ));
            }
            let destination = prospective_managed_file(installation_root, &new.path)?;
            match fs::symlink_metadata(&destination) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(transaction(
                        "transaction add path collides with an existing file",
                    ));
                }
                Err(_) => {
                    return Err(transaction("failed to inspect transaction add destination"));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn copy_verified_file(
    source: &Path,
    destination: &Path,
    expected: &ManagedFile,
) -> Result<(), UpdaterError> {
    let metadata = fs::metadata(source)
        .map_err(|_| transaction("failed to inspect transaction source file"))?;
    if metadata.len() != expected.size {
        return Err(transaction(
            "transaction source file size does not match the signed manifest",
        ));
    }
    let mut input =
        File::open(source).map_err(|_| transaction("failed to open transaction source file"))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| transaction("failed to create transaction copy"))?;
    let digest = copy_exact_hash(&mut input, &mut output, expected.size)?;
    output
        .sync_all()
        .map_err(|_| transaction("failed to sync transaction copy"))?;
    drop(output);
    ensure_direct_file(destination)?;
    if digest != expected.sha256 {
        return Err(transaction(
            "transaction source file digest does not match the signed manifest",
        ));
    }
    sync_parent(destination).map_err(|_| transaction("failed to sync transaction copy metadata"))
}
