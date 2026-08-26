//! The single permanent backup an installation keeps, and its promotion out of
//! a completed transaction.
//!
//! One backup, verified before it is trusted and consumed when it is used:
//! `upgrade --rollback` has exactly one place to go back to.

use std::{
    fs::{self},
    path::{Path, PathBuf},
};

use ah_updater_core::{
    InstallationIdentityV1, ReleaseManifest, ReleaseTrust, TransactionStateV1, UpdateOperation,
    UpdaterError,
};
use uuid::Uuid;

use super::fsverify::{
    create_synced_directory, ensure_direct_directory, ensure_directory_exists, path_exists,
    path_from_slashes, paths_equal, read_bounded, remove_known_directory,
    remove_optional_known_directory, sync_parent, write_new_synced,
};
use super::layout::{
    FILES_DIRECTORY, IDENTITY_FILE, INSTALLED_MANIFEST_FILE, INSTALLED_SIGNATURE_FILE,
    MAX_IDENTITY_BYTES, PERMANENT_BACKUP_DIRECTORY, TransactionPaths,
};
use super::managed::{FileMutationPhase, copy_manifest_files_with_injector, verify_manifest_files};
use super::record::{decode_identity, read_release_record};
use super::{
    LoadedTransaction, NoFailureInjection, TransactionRunError, load_transaction, recovery,
    remove_completed_transaction,
};

#[derive(Debug, Clone)]
pub struct PermanentBackup {
    pub(super) root: PathBuf,
    pub(super) identity: InstallationIdentityV1,
    pub(super) manifest: ReleaseManifest,
    pub(super) manifest_bytes: Vec<u8>,
    pub(super) signature_bytes: Vec<u8>,
}

pub fn load_permanent_backup(
    installation_root: &Path,
    installation_state_root: &Path,
    installation_id: Uuid,
    trust: &ReleaseTrust,
) -> Result<PermanentBackup, UpdaterError> {
    let root = permanent_backup_parent(installation_state_root, installation_id)?
        .join(installation_id.to_string());
    load_permanent_backup_at(&root, installation_root, installation_id, trust)
}

pub fn finalize_completed_transaction(
    paths: &TransactionPaths,
    trust: &ReleaseTrust,
) -> Result<TransactionStateV1, UpdaterError> {
    let transaction = load_transaction(paths, trust)?;
    match transaction.journal.state {
        TransactionStateV1::Committed => match transaction.plan.operation {
            UpdateOperation::Upgrade | UpdateOperation::Version => {
                promote_permanent_backup(&transaction, trust)?;
            }
            UpdateOperation::Rollback => {
                remove_permanent_backup(&transaction, trust)?;
            }
            UpdateOperation::Check | UpdateOperation::Recovery => {
                return Err(recovery("completed transaction operation is invalid"));
            }
        },
        TransactionStateV1::BackupPrepared | TransactionStateV1::RolledBack => {}
        _ => {
            return Err(recovery(
                "transaction cannot be finalized before reaching a safe terminal state",
            ));
        }
    }
    remove_completed_transaction(paths, trust)
}

pub(super) fn promote_permanent_backup(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let installation_id = transaction.plan.installation_id;
    let parent =
        permanent_backup_parent(transaction.paths.installation_state_root(), installation_id)?;
    ensure_directory_exists(&parent)?;
    let active = parent.join(installation_id.to_string());
    let staged = parent.join(format!("{installation_id}.new"));
    let previous = parent.join(format!("{installation_id}.old"));

    if path_exists(&active)? && permanent_backup_matches(&active, transaction, trust)? {
        remove_optional_known_directory(&staged)?;
        remove_optional_known_directory(&previous)?;
        return Ok(());
    }

    if path_exists(&previous)? {
        if path_exists(&active)? || !path_exists(&staged)? {
            return Err(recovery("permanent backup rotation state is ambiguous"));
        }
        fs::rename(&staged, &active)
            .map_err(|_| recovery("failed to complete permanent backup rotation"))?;
        sync_parent(&active).map_err(|_| recovery("failed to sync permanent backup rotation"))?;
        remove_known_directory(&previous)?;
        return verify_promoted_backup(&active, transaction, trust);
    }

    prepare_permanent_backup_stage(&staged, transaction, trust)?;
    if path_exists(&active)? {
        load_permanent_backup_at(
            &active,
            transaction.paths.installation_root(),
            installation_id,
            trust,
        )?;
        fs::rename(&active, &previous)
            .map_err(|_| recovery("failed to preserve the previous permanent backup"))?;
        sync_parent(&previous).map_err(|_| recovery("failed to sync permanent backup rotation"))?;
    }
    if let Err(error) = fs::rename(&staged, &active) {
        if path_exists(&previous)? {
            let _ = fs::rename(&previous, &active);
            let _ = sync_parent(&active);
        }
        let _ = error;
        return Err(recovery("failed to publish the new permanent backup"));
    }
    sync_parent(&active).map_err(|_| recovery("failed to sync the new permanent backup"))?;
    verify_promoted_backup(&active, transaction, trust)?;
    remove_optional_known_directory(&previous)
}

pub(super) fn prepare_permanent_backup_stage(
    staged: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    if path_exists(staged)? {
        if permanent_backup_matches(staged, transaction, trust).unwrap_or(false) {
            return Ok(());
        }
        remove_known_directory(staged)?;
    }
    create_synced_directory(staged)?;
    create_synced_directory(&staged.join(FILES_DIRECTORY))?;
    let source = transaction.paths.backup_root();
    for name in [
        IDENTITY_FILE,
        INSTALLED_MANIFEST_FILE,
        INSTALLED_SIGNATURE_FILE,
    ] {
        let limit = match name {
            IDENTITY_FILE => MAX_IDENTITY_BYTES,
            INSTALLED_MANIFEST_FILE => ah_updater_core::MAX_MANIFEST_BYTES,
            INSTALLED_SIGNATURE_FILE => ah_updater_core::DETACHED_SIGNATURE_BYTES,
            _ => unreachable!(),
        };
        let bytes = read_bounded(
            &source.join(name),
            limit,
            "failed to read transaction backup",
        )?;
        write_new_synced(&staged.join(name), &bytes)?;
    }
    let mut injector = NoFailureInjection;
    copy_manifest_files_with_injector(
        &mut injector,
        FileMutationPhase::BackupManaged,
        &transaction.paths.backup_files_root(),
        &staged.join(FILES_DIRECTORY),
        &transaction.old_manifest,
    )
    .map_err(TransactionRunError::into_updater_error)?;
    verify_promoted_backup(staged, transaction, trust)
}

pub(super) fn verify_promoted_backup(
    root: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let backup = load_permanent_backup_at(
        root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    if backup.identity != transaction.identity || backup.manifest != transaction.old_manifest {
        return Err(recovery(
            "permanent backup does not match the committed transaction",
        ));
    }
    Ok(())
}

pub(super) fn permanent_backup_matches(
    root: &Path,
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<bool, UpdaterError> {
    let backup = load_permanent_backup_at(
        root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    Ok(backup.identity == transaction.identity && backup.manifest == transaction.old_manifest)
}

pub(super) fn remove_permanent_backup(
    transaction: &LoadedTransaction,
    trust: &ReleaseTrust,
) -> Result<(), UpdaterError> {
    let root = permanent_backup_parent(
        transaction.paths.installation_state_root(),
        transaction.plan.installation_id,
    )?
    .join(transaction.plan.installation_id.to_string());
    if !path_exists(&root)? {
        return Ok(());
    }
    let backup = load_permanent_backup_at(
        &root,
        transaction.paths.installation_root(),
        transaction.plan.installation_id,
        trust,
    )?;
    if backup.manifest != transaction.new_manifest {
        return Err(recovery(
            "consumed permanent backup does not match the rollback target",
        ));
    }
    remove_known_directory(backup.root())
}

pub(super) fn load_permanent_backup_at(
    root: &Path,
    installation_root: &Path,
    installation_id: Uuid,
    trust: &ReleaseTrust,
) -> Result<PermanentBackup, UpdaterError> {
    ensure_direct_directory(root)?;
    ensure_direct_directory(&root.join(FILES_DIRECTORY))?;
    let identity_bytes = read_bounded(
        &root.join(IDENTITY_FILE),
        MAX_IDENTITY_BYTES,
        "failed to read permanent backup identity",
    )?;
    let identity = decode_identity(&identity_bytes)?;
    let record = read_release_record(root, trust)?;
    verify_manifest_files(&root.join(FILES_DIRECTORY), &record.manifest)?;
    if identity.installation_id != installation_id {
        return Err(recovery("permanent backup belongs to another installation"));
    }
    let executable = Path::new(&identity.executable_path);
    if !record.manifest.files.iter().any(|file| {
        paths_equal(
            executable,
            &installation_root.join(path_from_slashes(&file.path)),
        )
    }) {
        return Err(recovery(
            "permanent backup identity does not match the installation",
        ));
    }
    Ok(PermanentBackup {
        root: root.to_path_buf(),
        identity,
        manifest: record.manifest,
        manifest_bytes: record.manifest_bytes,
        signature_bytes: record.signature_bytes,
    })
}

pub(super) fn permanent_backup_parent(
    installation_state_root: &Path,
    installation_id: Uuid,
) -> Result<PathBuf, UpdaterError> {
    if installation_state_root
        .file_name()
        .and_then(|name| name.to_str())
        != Some(installation_id.to_string().as_str())
    {
        return Err(recovery("installation state root identity is invalid"));
    }
    let installations = installation_state_root
        .parent()
        .ok_or_else(|| recovery("installation state root has no parent"))?;
    let updater = installations
        .parent()
        .ok_or_else(|| recovery("installation state root is incomplete"))?;
    let product = updater
        .parent()
        .ok_or_else(|| recovery("installation state root is incomplete"))?;
    if installations.file_name().and_then(|name| name.to_str()) != Some("installations")
        || updater.file_name().and_then(|name| name.to_str()) != Some("updater")
    {
        return Err(recovery("installation state root layout is invalid"));
    }
    for directory in [product, updater, installations, installation_state_root] {
        ensure_direct_directory(directory)?;
    }
    Ok(product.join(PERMANENT_BACKUP_DIRECTORY))
}
